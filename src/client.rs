use crate::cli;
use crate::protocol;
use crate::pty::PtySize;
use std::io::{self, Read, Write};
use std::os::raw::c_int;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthChar;

static WINCH_PENDING: AtomicBool = AtomicBool::new(false);
static MOUSE_MODE_DEPTH: AtomicUsize = AtomicUsize::new(0);
const ENABLE_MOUSE_MODE: &[u8] = b"\x1b[?1000h\x1b[?1002h\x1b[?1006h";
const DISABLE_MOUSE_MODE: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b[?1000l";
const ENTER_ALTERNATE_SCREEN: &[u8] = b"\x1b[?1049h\x1b[?25l";
const EXIT_ALTERNATE_SCREEN: &[u8] = b"\x1b[?25h\x1b[?1049l";
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";
const HIDE_CURSOR: &[u8] = b"\x1b[?25l";
const LIVE_SNAPSHOT_REDRAW_INTERVAL: Duration = Duration::from_millis(100);
const LIVE_SNAPSHOT_EVENT_SAFETY_REDRAW_INTERVAL: Duration = Duration::from_millis(1000);
const PANE_NUMBER_DISPLAY_DURATION: Duration = Duration::from_millis(1000);
const COPY_MODE_ESCAPE_DISAMBIGUATION: Duration = Duration::from_millis(30);
const ATTACH_RESIZE_AMOUNT: usize = 5;
const CLEAR_SCREEN: &[u8] = b"\x1b[2J\x1b[H";
const CURSOR_HOME: &[u8] = b"\x1b[H";
const CLEAR_LINE: &[u8] = b"\x1b[2K";
const RESET_STYLE: &[u8] = b"\x1b[0m";
const SAVE_CURSOR: &[u8] = b"\x1b7";
const RESTORE_CURSOR: &[u8] = b"\x1b8";
const ATTACH_RENDER_RESPONSE: &str = "OK\tRENDER_OUTPUT_META\n";
const STDIN_FILENO: c_int = 0;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;

#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: c_int = 0o4000;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
const O_NONBLOCK: c_int = 0x0004;

unsafe extern "C" {
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachMode {
    Live { raw_layout_epoch: u64 },
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachRenderFrame {
    output: Vec<u8>,
    regions: Vec<AttachPaneRegion>,
    header_rows: usize,
}

#[derive(Debug, Default)]
struct LiveRenderOutputState {
    rows: Option<Vec<Vec<u8>>>,
    cursor: Vec<u8>,
    geometry: Option<LiveRenderOutputGeometry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedLiveRenderOutput {
    rows: Vec<Vec<u8>>,
    cursor: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveRenderOutputGeometry {
    header_rows: usize,
    regions: Vec<AttachPaneRegion>,
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
    let mut raw_layout_epoch = match parse_attach_ok(&response)? {
        AttachMode::Snapshot => {
            let _guard = RawModeGuard::enable();
            install_winch_handler();
            let mut last_size = initial_size;
            return run_live_snapshot_attach(
                socket,
                session,
                &mut stream,
                Vec::new(),
                &mut last_size,
                &mut on_resize,
            );
        }
        AttachMode::Live { raw_layout_epoch } => raw_layout_epoch,
    };

    let _guard = RawModeGuard::enable();
    install_winch_handler();
    let mut last_size = initial_size;
    let copy_mode_socket = socket.to_path_buf();
    let copy_mode_session = session.to_string();
    let mut pending_input = Vec::new();

    loop {
        write_attach_status_line(socket, session)?;
        let worker_copy_mode_socket = copy_mode_socket.clone();
        let worker_copy_mode_session = copy_mode_session.clone();
        let raw_exit = run_raw_attach_session(
            &mut stream,
            socket,
            session,
            raw_layout_epoch,
            std::mem::take(&mut pending_input),
            || {
                if take_winch_pending() {
                    maybe_emit_resize(detect_attach_size(), &mut last_size, &mut on_resize)?;
                }
                Ok(())
            },
            move |initial_input| {
                run_copy_mode(
                    &worker_copy_mode_socket,
                    &worker_copy_mode_session,
                    initial_input,
                )
            },
        )?;
        let _ = stream.shutdown(std::net::Shutdown::Both);

        let RawAttachExit::Reconnect {
            pending_input: pending_after_transition,
        } = raw_exit
        else {
            return Ok(());
        };

        stream = UnixStream::connect(socket)?;
        stream.write_all(protocol::encode_attach(session).as_bytes())?;
        let response = read_line(&mut stream)?;
        if let Some(message) = response.strip_prefix("ERR ") {
            return Err(io::Error::other(message.trim_end().to_string()));
        }
        match parse_attach_ok(&response)? {
            AttachMode::Snapshot => {
                install_winch_handler();
                return run_live_snapshot_attach(
                    socket,
                    session,
                    &mut stream,
                    pending_after_transition,
                    &mut last_size,
                    &mut on_resize,
                );
            }
            AttachMode::Live {
                raw_layout_epoch: next_raw_layout_epoch,
            } => {
                // Continue the raw loop on the fresh stream; pending bytes must still
                // pass through the raw prefix translator.
                raw_layout_epoch = next_raw_layout_epoch;
                pending_input = pending_after_transition;
            }
        }
    }
}

fn copy_attach_output_once(output_stream: &mut UnixStream) -> io::Result<bool> {
    let mut buf = [0_u8; 8192];
    let n = match output_stream.read(&mut buf) {
        Ok(0) => return Ok(false),
        Ok(n) => n,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(true);
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::InvalidInput
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };

    let mut stdout = io::stdout().lock();
    stdout.write_all(&buf[..n])?;
    stdout.flush()?;
    Ok(true)
}

fn run_raw_attach_session<F, C>(
    stream: &mut UnixStream,
    socket: &Path,
    session: &str,
    raw_layout_epoch: u64,
    initial_input: Vec<u8>,
    mut tick: F,
    enter_copy_mode: C,
) -> io::Result<RawAttachExit>
where
    F: FnMut() -> io::Result<()>,
    C: FnMut(&[u8]) -> io::Result<()> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let stop_input = Arc::new(AtomicBool::new(false));
    let copy_mode_active = Arc::new(AtomicBool::new(false));
    let mut input_stream = stream.try_clone()?;
    let input_socket = socket.to_path_buf();
    let input_session = session.to_string();
    let input_stop = Arc::clone(&stop_input);
    let input_copy_mode_active = Arc::clone(&copy_mode_active);
    std::thread::spawn(move || {
        let result = forward_stdin_until_detach(
            &mut input_stream,
            &input_socket,
            &input_session,
            initial_input,
            input_stop,
            input_copy_mode_active,
            || Ok(()),
            enter_copy_mode,
        );
        let _ = sender.send(result);
    });

    stream.set_read_timeout(Some(Duration::from_millis(50)))?;
    loop {
        tick()?;
        if let Ok(result) = receiver.try_recv() {
            let _ = stream.set_read_timeout(None);
            return result;
        }
        if !copy_attach_output_once(stream)? {
            let _ = stream.set_read_timeout(None);
            if let Ok(result) = receiver.recv_timeout(Duration::from_millis(50)) {
                return result;
            }
            let should_reconnect = raw_attach_should_reconnect(socket, session, raw_layout_epoch);
            stop_input.store(true, Ordering::SeqCst);
            let stopped_input = if should_reconnect && copy_mode_active.load(Ordering::SeqCst) {
                receiver.recv().ok()
            } else {
                receiver.recv_timeout(Duration::from_millis(250)).ok()
            };
            return if should_reconnect {
                match stopped_input {
                    Some(Ok(RawAttachExit::Reconnect { pending_input })) => {
                        Ok(RawAttachExit::Reconnect { pending_input })
                    }
                    _ => Ok(RawAttachExit::Reconnect {
                        pending_input: Vec::new(),
                    }),
                }
            } else {
                Ok(RawAttachExit::Detach)
            };
        }
    }
}

fn raw_attach_should_reconnect(socket: &Path, session: &str, raw_layout_epoch: u64) -> bool {
    read_raw_attach_layout_epoch(socket, session)
        .is_ok_and(|current_epoch| current_epoch > raw_layout_epoch)
}

fn read_raw_attach_layout_epoch(socket: &Path, session: &str) -> io::Result<u64> {
    let body = send_control_request(socket, &protocol::encode_attach_raw_state(session))?;
    let body = String::from_utf8_lossy(&body);
    let Some(line) = body.lines().next() else {
        return Err(io::Error::other("missing raw attach state"));
    };
    let Some(epoch) = line.strip_prefix("RAW_LAYOUT_EPOCH\t") else {
        return Err(io::Error::other("invalid raw attach state"));
    };
    epoch
        .parse::<u64>()
        .map_err(|_| io::Error::other("invalid raw attach layout epoch"))
}

fn read_attach_status_line(socket: &Path, session: &str) -> io::Result<String> {
    let _ = (socket, session);
    Ok(String::new())
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

fn read_attach_layout_frame(
    socket: &Path,
    session: &str,
) -> io::Result<AttachLayoutSnapshotResponse> {
    match send_control_request(socket, &protocol::encode_attach_layout_frame(session)) {
        Ok(body) => parse_attach_layout_snapshot_response(&body),
        Err(error) if is_unknown_request_error(&error) => {
            read_attach_layout_snapshot(socket, session)
        }
        Err(error) => Err(error),
    }
}

fn read_attach_render_frame(stream: &mut UnixStream) -> io::Result<AttachRenderFrame> {
    let header = read_line(stream)?;
    let Some(len) = header
        .strip_prefix("FRAME\t")
        .and_then(|line| line.strip_suffix('\n'))
    else {
        return Err(io::Error::other("invalid attach render frame header"));
    };
    let len = len
        .parse::<usize>()
        .map_err(|_| io::Error::other("invalid attach render frame length"))?;
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body)?;
    parse_attach_render_frame_body(&body)
}

fn parse_attach_render_frame_body(body: &[u8]) -> io::Result<AttachRenderFrame> {
    let mut cursor = 0;
    let header_rows = read_body_line(body, &mut cursor)?;
    let Some(header_rows) = header_rows.strip_prefix("HEADER_ROWS\t") else {
        return Err(io::Error::other("missing render header rows"));
    };
    let header_rows = header_rows
        .parse::<usize>()
        .map_err(|_| io::Error::other("invalid render header rows"))?;

    let regions = read_body_line(body, &mut cursor)?;
    let Some(count) = regions.strip_prefix("REGIONS\t") else {
        return Err(io::Error::other("missing render regions header"));
    };
    let count = count
        .parse::<usize>()
        .map_err(|_| io::Error::other("invalid render region count"))?;

    let mut regions = Vec::new();
    for _ in 0..count {
        let line = read_body_line(body, &mut cursor)?;
        regions.push(parse_attach_pane_region(line)?);
    }

    let output = read_body_line(body, &mut cursor)?;
    let Some(len) = output.strip_prefix("OUTPUT\t") else {
        return Err(io::Error::other("missing render output header"));
    };
    let len = len
        .parse::<usize>()
        .map_err(|_| io::Error::other("invalid render output length"))?;
    let remaining = body.len().saturating_sub(cursor);
    if remaining < len {
        return Err(io::Error::other("truncated render output body"));
    }
    if remaining > len {
        return Err(io::Error::other("extra render output body bytes"));
    }

    Ok(AttachRenderFrame {
        output: body[cursor..cursor + len].to_vec(),
        regions,
        header_rows,
    })
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
    for line in status.lines() {
        stdout.write_all(line.as_bytes())?;
        stdout.write_all(b"\r\n")?;
    }
    stdout.flush()
}

fn parse_attach_ok(response: &str) -> io::Result<AttachMode> {
    if response == "OK\n" {
        return Ok(AttachMode::Live {
            raw_layout_epoch: 0,
        });
    }

    if response == "OK\tSNAPSHOT\n" {
        return Ok(AttachMode::Snapshot);
    }

    if let Some(epoch) = response
        .strip_prefix("OK\tLIVE\t")
        .and_then(|value| value.strip_suffix('\n'))
    {
        let raw_layout_epoch = epoch
            .parse::<u64>()
            .map_err(|_| io::Error::other("invalid raw attach layout epoch"))?;
        return Ok(AttachMode::Live { raw_layout_epoch });
    }

    Err(io::Error::other(format!(
        "unexpected server response: {response:?}"
    )))
}

fn write_initial_live_snapshot_frame(
    socket: &Path,
    session: &str,
) -> io::Result<LiveSnapshotFrame> {
    write_live_snapshot_frame_with_message_and_clear(socket, session, None, None, true, false)
}

fn write_live_snapshot_frame_with_message_and_clear(
    socket: &Path,
    session: &str,
    message: Option<&str>,
    overlay: Option<&str>,
    clear: bool,
    prioritize_message: bool,
) -> io::Result<LiveSnapshotFrame> {
    let status = read_attach_status_line(socket, session)?;
    let snapshot = read_attach_layout_frame(socket, session)?;
    write_live_frame_to_stdout(
        &status,
        message,
        overlay,
        &snapshot,
        clear,
        prioritize_message,
    )
}

fn write_live_frame_to_stdout(
    status: &str,
    message: Option<&str>,
    overlay: Option<&str>,
    snapshot: &AttachLayoutSnapshotResponse,
    clear: bool,
    prioritize_message: bool,
) -> io::Result<LiveSnapshotFrame> {
    let attach_size = detect_attach_size().or_else(|| overlay.map(|_| default_attach_size()));
    let width = attach_size.map(|size| usize::from(size.cols));
    let max_header_rows = attach_size.map(|size| {
        let rows = usize::from(size.rows);
        if rows > 1 { rows - 1 } else { rows }
    });
    let header_lines = cap_live_header_lines(
        live_header_lines(status, message, width),
        max_header_rows,
        width,
        prioritize_message && message.is_some(),
    );
    let header_rows = header_lines.len();
    let snapshot_rows = attach_size
        .map(|size| usize::from(size.rows).saturating_sub(header_rows))
        .unwrap_or(usize::MAX);

    let mut stdout = io::stdout().lock();
    if clear {
        stdout.write_all(CLEAR_SCREEN)?;
    } else {
        stdout.write_all(CURSOR_HOME)?;
    }
    for line in header_lines {
        write_repaint_line(&mut stdout, line.as_bytes(), !clear)?;
    }
    if clear {
        write_snapshot_rows(&mut stdout, &snapshot.snapshot, snapshot_rows)?;
    } else {
        write_snapshot_rows_for_repaint(&mut stdout, &snapshot.snapshot, snapshot_rows)?;
    }
    if let (Some(overlay), Some(size)) = (overlay, attach_size) {
        append_help_popup_overlay(&mut stdout, overlay, size, header_rows)?;
    }
    stdout.flush()?;

    Ok(LiveSnapshotFrame {
        regions: snapshot.regions.clone(),
        header_rows,
    })
}

fn live_header_lines(status: &str, message: Option<&str>, width: Option<usize>) -> Vec<String> {
    let mut lines = Vec::new();
    if !status.is_empty() {
        for line in status.lines() {
            lines.push(truncate_header_line(line, width));
        }
    }
    if let Some(message) = message {
        for line in message.lines() {
            lines.push(truncate_header_line(line, width));
        }
    }
    lines
}

fn cap_live_header_lines(
    mut lines: Vec<String>,
    max_rows: Option<usize>,
    width: Option<usize>,
    prioritize_tail: bool,
) -> Vec<String> {
    let Some(max_rows) = max_rows else {
        return lines;
    };
    if lines.len() <= max_rows {
        return lines;
    }
    if max_rows == 0 {
        return Vec::new();
    }
    if prioritize_tail {
        return lines[lines.len() - max_rows..].to_vec();
    }

    let omitted = lines.len() - max_rows + 1;
    lines.truncate(max_rows);
    lines[max_rows - 1] = truncate_header_line(&format!("... {omitted} more lines"), width);
    lines
}

fn truncate_header_line(line: &str, width: Option<usize>) -> String {
    let line = line.trim_end_matches('\r');
    let Some(width) = width else {
        return line.to_string();
    };
    if width == 0 {
        return String::new();
    }
    if display_cell_width(line) <= width {
        return line.to_string();
    }
    if width <= 3 {
        return take_cells(line, width);
    }
    let mut output = take_cells(line, width - 3);
    output.push_str("...");
    output
}

fn display_cell_width(line: &str) -> usize {
    line.chars().map(char_cell_width).sum()
}

fn take_cells(line: &str, max_cells: usize) -> String {
    let mut output = String::new();
    let mut used = 0;
    for ch in line.chars() {
        let width = char_cell_width(ch);
        if used + width > max_cells {
            break;
        }
        output.push(ch);
        used += width;
    }
    output
}

fn char_cell_width(ch: char) -> usize {
    UnicodeWidthChar::width(ch).unwrap_or(1).max(1)
}

fn write_repaint_line(stdout: &mut impl Write, line: &[u8], clear_line: bool) -> io::Result<()> {
    if clear_line {
        stdout.write_all(CLEAR_LINE)?;
    }
    stdout.write_all(line)?;
    stdout.write_all(b"\r\n")
}

fn write_snapshot_rows(
    stdout: &mut impl Write,
    snapshot: &[u8],
    max_rows: usize,
) -> io::Result<()> {
    if max_rows == 0 {
        return Ok(());
    }

    let mut rows = 0;
    let mut start = 0;
    while start < snapshot.len() && rows < max_rows {
        let line_end = snapshot[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(snapshot.len(), |offset| start + offset);
        let content_end = if line_end > start && snapshot[line_end - 1] == b'\r' {
            line_end - 1
        } else {
            line_end
        };

        if rows > 0 {
            stdout.write_all(b"\r\n")?;
        }
        stdout.write_all(&snapshot[start..content_end])?;
        rows += 1;

        if line_end == snapshot.len() {
            break;
        }
        start = line_end + 1;
    }

    Ok(())
}

fn write_live_render_output(
    frame: &AttachRenderFrame,
    state: &mut LiveRenderOutputState,
    overlay: Option<&str>,
) -> io::Result<()> {
    let mut output = diff_live_render_output(frame, state);
    if let Some(overlay) = overlay {
        let size = detect_attach_size().unwrap_or_else(default_attach_size);
        append_centered_popup_overlay(
            &mut output,
            "dmux help",
            &popup_content_lines(overlay),
            size,
            frame.header_rows,
        );
    }
    let mut stdout = io::stdout().lock();
    stdout.write_all(&output)?;
    stdout.flush()
}

fn default_attach_size() -> PtySize {
    PtySize { cols: 80, rows: 24 }
}

fn append_help_popup_overlay(
    stdout: &mut impl Write,
    overlay: &str,
    size: PtySize,
    header_rows: usize,
) -> io::Result<()> {
    let mut output = Vec::new();
    append_centered_popup_overlay(
        &mut output,
        "dmux help",
        &popup_content_lines(overlay),
        size,
        header_rows,
    );
    stdout.write_all(&output)
}

fn append_centered_popup_overlay(
    output: &mut Vec<u8>,
    title: &str,
    content: &[String],
    size: PtySize,
    header_rows: usize,
) {
    let rows = usize::from(size.rows);
    let cols = usize::from(size.cols);
    if rows <= header_rows || cols < 4 {
        return;
    }

    let available_rows = rows.saturating_sub(header_rows);
    if available_rows < 3 {
        return;
    }

    let popup_lines = boxed_popup_lines(title, content, Some(cols), Some(available_rows));
    if popup_lines.is_empty() {
        return;
    }

    let popup_height = popup_lines.len();
    let popup_width = popup_lines
        .iter()
        .map(|line| display_cell_width(line))
        .max()
        .unwrap_or(0);
    if popup_height > available_rows || popup_width > cols {
        return;
    }

    let top = header_rows + ((available_rows.saturating_sub(popup_height) + 1) / 2) + 1;
    let left = ((cols.saturating_sub(popup_width) + 1) / 2) + 1;

    output.extend_from_slice(SAVE_CURSOR);
    for (index, line) in popup_lines.iter().enumerate() {
        write_absolute_cursor_position(output, top + index, left);
        output.extend_from_slice(RESET_STYLE);
        output.extend_from_slice(line.as_bytes());
    }
    output.extend_from_slice(RESTORE_CURSOR);
}

fn popup_content_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn boxed_popup_lines(
    title: &str,
    content: &[String],
    max_total_width: Option<usize>,
    max_rows: Option<usize>,
) -> Vec<String> {
    if max_total_width.is_some_and(|width| width < 4) {
        return Vec::new();
    }

    let max_content_rows = max_rows
        .map(|rows| rows.saturating_sub(2))
        .unwrap_or(usize::MAX);
    if max_content_rows == 0 {
        return Vec::new();
    }

    let mut visible_content = content
        .iter()
        .take(max_content_rows)
        .cloned()
        .collect::<Vec<_>>();
    if content.len() > max_content_rows {
        let replacement = if max_content_rows == 1 {
            "...".to_string()
        } else {
            "... more".to_string()
        };
        if let Some(last) = visible_content.last_mut() {
            *last = replacement;
        }
    }

    let max_inner_width = max_total_width
        .map(|width| width.saturating_sub(2))
        .unwrap_or(usize::MAX);
    let max_content_width = max_inner_width.saturating_sub(2);
    let content_width = visible_content
        .iter()
        .map(|line| display_cell_width(line))
        .max()
        .unwrap_or(0)
        .min(max_content_width);
    let title_text = format!(" {title} ");
    let mut border_width = (content_width + 2).max(display_cell_width(&title_text));
    border_width = border_width.min(max_inner_width);
    if border_width == 0 {
        return Vec::new();
    }

    let title_segment = truncate_header_line(&title_text, Some(border_width));
    let trailing = border_width.saturating_sub(display_cell_width(&title_segment));
    let inner_width = border_width.saturating_sub(2);
    let mut lines = Vec::with_capacity(visible_content.len() + 2);
    lines.push(format!("┌{title_segment}{}┐", "─".repeat(trailing)));
    for line in visible_content {
        let line = truncate_header_line(&line, Some(inner_width));
        lines.push(format!("│ {line}{} │", cell_padding(&line, inner_width)));
    }
    lines.push(format!("└{}┘", "─".repeat(border_width)));
    lines
}

fn reset_live_render_output_state(state: &mut LiveRenderOutputState) {
    state.rows = None;
    state.cursor.clear();
    state.geometry = None;
}

fn diff_live_render_output(
    frame: &AttachRenderFrame,
    state: &mut LiveRenderOutputState,
) -> Vec<u8> {
    let Some(parsed) = parse_live_render_output(&frame.output) else {
        reset_live_render_output_state(state);
        return full_live_render_output(&frame.output);
    };
    let geometry = LiveRenderOutputGeometry {
        header_rows: frame.header_rows,
        regions: frame.regions.clone(),
    };

    let Some(previous_rows) = state.rows.as_ref() else {
        store_live_render_output_state(state, geometry, parsed);
        return full_live_render_output(&frame.output);
    };
    if previous_rows.len() != parsed.rows.len()
        || state
            .geometry
            .as_ref()
            .is_none_or(|stored| stored != &geometry)
    {
        store_live_render_output_state(state, geometry, parsed);
        return full_live_render_output(&frame.output);
    }

    let mut output = Vec::new();
    let mut changed_rows = false;
    for (row, current) in parsed.rows.iter().enumerate() {
        if previous_rows
            .get(row)
            .is_none_or(|previous| previous != current)
        {
            write_absolute_cursor_position(&mut output, row + 1, 1);
            output.extend_from_slice(RESET_STYLE);
            output.extend_from_slice(current);
            changed_rows = true;
        }
    }
    if changed_rows || state.cursor != parsed.cursor {
        output.extend_from_slice(&parsed.cursor);
    }
    store_live_render_output_state(state, geometry, parsed);
    output
}

fn full_live_render_output(output: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(RESET_STYLE.len() + output.len());
    framed.extend_from_slice(RESET_STYLE);
    framed.extend_from_slice(output);
    framed
}

fn store_live_render_output_state(
    state: &mut LiveRenderOutputState,
    geometry: LiveRenderOutputGeometry,
    parsed: ParsedLiveRenderOutput,
) {
    state.rows = Some(parsed.rows);
    state.cursor = parsed.cursor;
    state.geometry = Some(geometry);
}

fn parse_live_render_output(render_output: &[u8]) -> Option<ParsedLiveRenderOutput> {
    let body = render_output.strip_prefix(CURSOR_HOME)?;
    let (rows, cursor) = split_trailing_cursor_position(body);
    Some(ParsedLiveRenderOutput {
        rows: split_render_output_rows(rows),
        cursor: cursor.to_vec(),
    })
}

fn split_trailing_cursor_position(output: &[u8]) -> (&[u8], &[u8]) {
    for start in (0..output.len())
        .rev()
        .filter(|index| output[*index] == b'\x1b')
    {
        if is_cursor_position_escape(&output[start..]) {
            let cursor_start = cursor_state_start(output, start);
            return (&output[..cursor_start], &output[cursor_start..]);
        }
    }
    (output, &[])
}

fn cursor_state_start(output: &[u8], cursor_position_start: usize) -> usize {
    let before_cursor = &output[..cursor_position_start];
    if before_cursor.ends_with(SHOW_CURSOR) {
        cursor_position_start - SHOW_CURSOR.len()
    } else if before_cursor.ends_with(HIDE_CURSOR) {
        cursor_position_start - HIDE_CURSOR.len()
    } else {
        cursor_position_start
    }
}

fn is_cursor_position_escape(bytes: &[u8]) -> bool {
    let Some(bytes) = bytes.strip_prefix(b"\x1b[") else {
        return false;
    };
    let Some((row, tail)) = split_digits(bytes) else {
        return false;
    };
    if row.is_empty() {
        return false;
    }
    let Some(tail) = tail.strip_prefix(b";") else {
        return false;
    };
    let Some((col, tail)) = split_digits(tail) else {
        return false;
    };
    !col.is_empty() && tail == b"H"
}

fn split_digits(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    Some((&bytes[..len], &bytes[len..]))
}

fn split_render_output_rows(rows: &[u8]) -> Vec<Vec<u8>> {
    rows.split(|byte| *byte == b'\n')
        .map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            line.to_vec()
        })
        .collect()
}

fn write_absolute_cursor_position(output: &mut Vec<u8>, row: usize, col: usize) {
    output.extend_from_slice(format!("\x1b[{row};{col}H").as_bytes());
}

fn write_snapshot_rows_for_repaint(
    stdout: &mut impl Write,
    snapshot: &[u8],
    max_rows: usize,
) -> io::Result<()> {
    if max_rows == 0 {
        return Ok(());
    }

    let mut rows = 0;
    let mut start = 0;
    while start < snapshot.len() && rows < max_rows {
        let line_end = snapshot[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(snapshot.len(), |offset| start + offset);
        let content_end = if line_end > start && snapshot[line_end - 1] == b'\r' {
            line_end - 1
        } else {
            line_end
        };

        if rows > 0 {
            stdout.write_all(b"\r\n")?;
        }
        stdout.write_all(CLEAR_LINE)?;
        stdout.write_all(&snapshot[start..content_end])?;
        rows += 1;

        if line_end == snapshot.len() {
            break;
        }
        start = line_end + 1;
    }

    Ok(())
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

fn maybe_handle_live_snapshot_resize<F>(
    last_size: &mut Option<PtySize>,
    on_resize: &mut F,
) -> io::Result<bool>
where
    F: FnMut(PtySize) -> io::Result<()>,
{
    if !take_winch_pending() {
        return Ok(false);
    }

    let before = *last_size;
    maybe_emit_resize(detect_attach_size(), last_size, on_resize)?;
    Ok(*last_size != before)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneCommand {
    NewWindow,
    NextWindow,
    PreviousWindow,
    SplitRight,
    SplitDown,
    FocusLeft,
    FocusDown,
    FocusUp,
    FocusRight,
    Resize {
        direction: protocol::PaneResizeDirection,
        amount: usize,
    },
    Close,
    ToggleZoom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AttachInputAction {
    Forward(Vec<u8>),
    PaneCommand(PaneCommand),
    CommandPromptStart,
    CommandPromptUpdate(String),
    CommandPromptDispatch {
        command: String,
        trailing_input: Vec<u8>,
    },
    CommandPromptCancel,
    SelectNextPane,
    ShowPaneNumbers,
    SelectPane(usize),
    EnterCopyMode {
        initial_input: Vec<u8>,
    },
    ShowHelp,
    Detach,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveSnapshotInputAction {
    Forward(Vec<u8>),
    MousePress(MousePosition),
    PaneCommand(PaneCommand),
    CommandPromptStart,
    CommandPromptUpdate(String),
    CommandPromptDispatch {
        command: String,
        trailing_input: Vec<u8>,
    },
    CommandPromptCancel,
    Detach,
    SelectNextPane,
    ShowPaneNumbers,
    ShowHelp,
    SelectPane(usize),
    EnterCopyMode {
        initial_input: Vec<u8>,
    },
}

#[derive(Default)]
struct LiveSnapshotInputState {
    saw_prefix: bool,
    selecting_pane: bool,
    command_prompt: Option<Vec<u8>>,
    mouse_pending: Vec<u8>,
    prompt_pending_since: Option<Instant>,
}

#[derive(Default)]
struct RawAttachInputState {
    saw_prefix: bool,
    selecting_pane: bool,
    command_prompt: Option<Vec<u8>>,
    mouse_pending: Vec<u8>,
    prompt_pending_since: Option<Instant>,
}

#[derive(Debug, Clone)]
struct LiveControls {
    prefix: u8,
    bindings: Vec<(crate::config::KeyStroke, LiveKeyAction)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveKeyAction {
    SendPrefix,
    Detach,
    CopyMode,
    SelectNextPane,
    ShowPaneNumbers,
    ShowHelp,
    CommandPrompt,
    PaneCommand(PaneCommand),
}

impl Default for LiveControls {
    fn default() -> Self {
        Self::from_entries(
            crate::config::DEFAULT_PREFIX_KEY,
            crate::config::default_key_bindings(),
        )
    }
}

impl LiveControls {
    fn from_entries(prefix: &str, bindings: Vec<protocol::KeyBinding>) -> Self {
        let prefix = crate::config::key_name_to_byte(prefix).unwrap_or(0x02);
        let bindings = bindings
            .into_iter()
            .filter_map(|binding| {
                let key = crate::config::parse_key_stroke(&binding.key).ok()?;
                let action = live_key_action(&binding.command)?;
                Some((key, action))
            })
            .collect();
        Self { prefix, bindings }
    }

    fn action_for_key(&self, key: crate::config::KeyStroke) -> Option<LiveKeyAction> {
        self.bindings.iter().find_map(|(binding_key, action)| {
            key_strokes_match(*binding_key, key).then_some(*action)
        })
    }

    fn key_for_action(&self, target: LiveKeyAction) -> Option<crate::config::KeyStroke> {
        self.bindings
            .iter()
            .find_map(|(key, action)| (*action == target).then_some(*key))
    }
}

fn key_strokes_match(binding: crate::config::KeyStroke, input: crate::config::KeyStroke) -> bool {
    binding.code == input.code
        && binding.alt == input.alt
        && binding.shift == input.shift
        && (binding.ctrl == input.ctrl
            || matches!(binding.code, crate::config::KeyCode::Byte(1..=26)))
}

fn live_key_action(command: &str) -> Option<LiveKeyAction> {
    if let Some(action) = resize_live_key_action(command) {
        return Some(action);
    }

    match command {
        "send-prefix" => Some(LiveKeyAction::SendPrefix),
        "detach-client" => Some(LiveKeyAction::Detach),
        "copy-mode" => Some(LiveKeyAction::CopyMode),
        "next-pane" => Some(LiveKeyAction::SelectNextPane),
        "display-panes" => Some(LiveKeyAction::ShowPaneNumbers),
        "show-help" => Some(LiveKeyAction::ShowHelp),
        "command-prompt" => Some(LiveKeyAction::CommandPrompt),
        "new-window" => Some(LiveKeyAction::PaneCommand(PaneCommand::NewWindow)),
        "next-window" => Some(LiveKeyAction::PaneCommand(PaneCommand::NextWindow)),
        "previous-window" => Some(LiveKeyAction::PaneCommand(PaneCommand::PreviousWindow)),
        "split-window -h" => Some(LiveKeyAction::PaneCommand(PaneCommand::SplitRight)),
        "split-window -v" => Some(LiveKeyAction::PaneCommand(PaneCommand::SplitDown)),
        "select-pane -L" => Some(LiveKeyAction::PaneCommand(PaneCommand::FocusLeft)),
        "select-pane -D" => Some(LiveKeyAction::PaneCommand(PaneCommand::FocusDown)),
        "select-pane -U" => Some(LiveKeyAction::PaneCommand(PaneCommand::FocusUp)),
        "select-pane -R" => Some(LiveKeyAction::PaneCommand(PaneCommand::FocusRight)),
        "kill-pane" => Some(LiveKeyAction::PaneCommand(PaneCommand::Close)),
        "zoom-pane" => Some(LiveKeyAction::PaneCommand(PaneCommand::ToggleZoom)),
        _ => None,
    }
}

fn resize_live_key_action(command: &str) -> Option<LiveKeyAction> {
    let mut parts = command.split_whitespace();
    let resize = parts.next()?;
    let direction = parts.next()?;
    if resize != "resize-pane" {
        return None;
    }
    let direction = match direction {
        "-L" => protocol::PaneResizeDirection::Left,
        "-D" => protocol::PaneResizeDirection::Down,
        "-U" => protocol::PaneResizeDirection::Up,
        "-R" => protocol::PaneResizeDirection::Right,
        _ => return None,
    };
    let amount = parts
        .next()
        .map(|amount| amount.parse::<usize>().ok())
        .unwrap_or(Some(ATTACH_RESIZE_AMOUNT))?;
    if amount == 0 || parts.next().is_some() {
        return None;
    }
    Some(LiveKeyAction::PaneCommand(PaneCommand::Resize {
        direction,
        amount,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParsedInputKey {
    Key(crate::config::KeyStroke, usize),
    Unhandled(usize),
    Incomplete,
}

fn parse_input_key(input: &[u8]) -> ParsedInputKey {
    let Some(byte) = input.first().copied() else {
        return ParsedInputKey::Incomplete;
    };
    if byte != 0x1b {
        return ParsedInputKey::Key(
            crate::config::KeyStroke {
                code: crate::config::KeyCode::Byte(byte),
                ctrl: false,
                alt: false,
                shift: false,
            },
            1,
        );
    }

    match input {
        [0x1b] => ParsedInputKey::Key(
            crate::config::KeyStroke {
                code: crate::config::KeyCode::Byte(0x1b),
                ctrl: false,
                alt: false,
                shift: false,
            },
            1,
        ),
        [0x1b, b'[', ..] => parse_csi_input_key(input),
        [0x1b, b'O'] => ParsedInputKey::Incomplete,
        [0x1b, b'O', final_byte, ..] => {
            if let Some(code) = arrow_key_code(*final_byte) {
                ParsedInputKey::Key(
                    crate::config::KeyStroke {
                        code,
                        ctrl: false,
                        alt: false,
                        shift: false,
                    },
                    3,
                )
            } else {
                ParsedInputKey::Unhandled(3)
            }
        }
        _ => match parse_input_key(&input[1..]) {
            ParsedInputKey::Key(mut key, consumed) => {
                key.alt = true;
                ParsedInputKey::Key(key, consumed + 1)
            }
            ParsedInputKey::Unhandled(consumed) => ParsedInputKey::Unhandled(consumed + 1),
            ParsedInputKey::Incomplete => ParsedInputKey::Key(
                crate::config::KeyStroke {
                    code: crate::config::KeyCode::Byte(0x1b),
                    ctrl: false,
                    alt: false,
                    shift: false,
                },
                1,
            ),
        },
    }
}

fn parse_csi_input_key(input: &[u8]) -> ParsedInputKey {
    let Some(consumed) = complete_csi_sequence_len(input) else {
        return if is_incomplete_csi_sequence(input) {
            ParsedInputKey::Incomplete
        } else {
            ParsedInputKey::Unhandled(input.len().min(1))
        };
    };
    let final_byte = input[consumed - 1];
    let Some(code) = arrow_key_code(final_byte) else {
        return ParsedInputKey::Unhandled(consumed);
    };
    let Some((shift, alt, ctrl)) = csi_modifier_state(&input[2..consumed - 1]) else {
        return ParsedInputKey::Unhandled(consumed);
    };
    ParsedInputKey::Key(
        crate::config::KeyStroke {
            code,
            ctrl,
            alt,
            shift,
        },
        consumed,
    )
}

fn arrow_key_code(final_byte: u8) -> Option<crate::config::KeyCode> {
    match final_byte {
        b'D' => Some(crate::config::KeyCode::Left),
        b'B' => Some(crate::config::KeyCode::Down),
        b'A' => Some(crate::config::KeyCode::Up),
        b'C' => Some(crate::config::KeyCode::Right),
        _ => None,
    }
}

fn csi_modifier_state(params: &[u8]) -> Option<(bool, bool, bool)> {
    if params.is_empty() {
        return Some((false, false, false));
    }
    let params = std::str::from_utf8(params).ok()?;
    if params.starts_with('<') {
        return None;
    }

    let values = params
        .split(';')
        .filter(|part| !part.is_empty())
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let modifier = if params.contains(';') {
        values.last().copied().unwrap_or(1)
    } else {
        values
            .first()
            .copied()
            .filter(|value| *value >= 2)
            .unwrap_or(1)
    };
    let encoded = modifier.checked_sub(1)?;
    if encoded > 7 {
        return None;
    }
    Some((encoded & 1 != 0, encoded & 2 != 0, encoded & 4 != 0))
}

fn key_stroke_input_bytes(key: crate::config::KeyStroke) -> Vec<u8> {
    match key.code {
        crate::config::KeyCode::Byte(byte) => {
            if key.alt {
                vec![0x1b, byte]
            } else {
                vec![byte]
            }
        }
        crate::config::KeyCode::Left
        | crate::config::KeyCode::Down
        | crate::config::KeyCode::Up
        | crate::config::KeyCode::Right => {
            let final_byte = match key.code {
                crate::config::KeyCode::Left => b'D',
                crate::config::KeyCode::Down => b'B',
                crate::config::KeyCode::Up => b'A',
                crate::config::KeyCode::Right => b'C',
                crate::config::KeyCode::Byte(_) => unreachable!(),
            };
            let modifier =
                1 + u8::from(key.shift) + (u8::from(key.alt) * 2) + (u8::from(key.ctrl) * 4);
            if modifier == 1 {
                vec![0x1b, b'[', final_byte]
            } else {
                format!("\x1b[1;{}{}", modifier, final_byte as char).into_bytes()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyActionFlow {
    Continue,
    Stop,
}

fn push_attach_key_action(
    actions: &mut Vec<AttachInputAction>,
    output: &mut Vec<u8>,
    state: &mut RawAttachInputState,
    controls: &LiveControls,
    action: LiveKeyAction,
    trailing_input: &[u8],
) -> KeyActionFlow {
    if !output.is_empty() {
        actions.push(AttachInputAction::Forward(std::mem::take(output)));
    }
    match action {
        LiveKeyAction::SendPrefix => output.push(controls.prefix),
        LiveKeyAction::Detach => {
            actions.push(AttachInputAction::Detach);
            return KeyActionFlow::Stop;
        }
        LiveKeyAction::CopyMode => {
            actions.push(AttachInputAction::EnterCopyMode {
                initial_input: trailing_input.to_vec(),
            });
            return KeyActionFlow::Stop;
        }
        LiveKeyAction::SelectNextPane => actions.push(AttachInputAction::SelectNextPane),
        LiveKeyAction::ShowPaneNumbers => {
            state.selecting_pane = true;
            actions.push(AttachInputAction::ShowPaneNumbers);
        }
        LiveKeyAction::ShowHelp => actions.push(AttachInputAction::ShowHelp),
        LiveKeyAction::CommandPrompt => {
            state.command_prompt = Some(Vec::new());
            actions.push(AttachInputAction::CommandPromptStart);
        }
        LiveKeyAction::PaneCommand(command) => {
            actions.push(AttachInputAction::PaneCommand(command))
        }
    }
    KeyActionFlow::Continue
}

fn push_live_snapshot_key_action(
    actions: &mut Vec<LiveSnapshotInputAction>,
    output: &mut Vec<u8>,
    state: &mut LiveSnapshotInputState,
    controls: &LiveControls,
    action: LiveKeyAction,
    trailing_input: &[u8],
) -> KeyActionFlow {
    if !output.is_empty() {
        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(output)));
    }
    match action {
        LiveKeyAction::SendPrefix => output.push(controls.prefix),
        LiveKeyAction::Detach => {
            actions.push(LiveSnapshotInputAction::Detach);
            return KeyActionFlow::Stop;
        }
        LiveKeyAction::CopyMode => {
            actions.push(LiveSnapshotInputAction::EnterCopyMode {
                initial_input: trailing_input.to_vec(),
            });
            return KeyActionFlow::Stop;
        }
        LiveKeyAction::SelectNextPane => actions.push(LiveSnapshotInputAction::SelectNextPane),
        LiveKeyAction::ShowPaneNumbers => {
            state.selecting_pane = true;
            actions.push(LiveSnapshotInputAction::ShowPaneNumbers);
        }
        LiveKeyAction::ShowHelp => actions.push(LiveSnapshotInputAction::ShowHelp),
        LiveKeyAction::CommandPrompt => {
            state.command_prompt = Some(Vec::new());
            actions.push(LiveSnapshotInputAction::CommandPromptStart);
        }
        LiveKeyAction::PaneCommand(command) => {
            actions.push(LiveSnapshotInputAction::PaneCommand(command));
        }
    }
    KeyActionFlow::Continue
}

impl RawAttachInputState {
    fn prompt_pending_timeout_action(&mut self, now: Instant) -> Option<AttachInputAction> {
        if prompt_pending_timeout(
            &mut self.command_prompt,
            &mut self.mouse_pending,
            &mut self.prompt_pending_since,
            now,
        ) {
            return Some(AttachInputAction::CommandPromptCancel);
        }
        None
    }
}

impl LiveSnapshotInputState {
    fn prompt_pending_timeout_action(&mut self, now: Instant) -> Option<LiveSnapshotInputAction> {
        if prompt_pending_timeout(
            &mut self.command_prompt,
            &mut self.mouse_pending,
            &mut self.prompt_pending_since,
            now,
        ) {
            return Some(LiveSnapshotInputAction::CommandPromptCancel);
        }
        None
    }
}

fn prompt_pending_timeout(
    command_prompt: &mut Option<Vec<u8>>,
    pending: &mut Vec<u8>,
    pending_since: &mut Option<Instant>,
    now: Instant,
) -> bool {
    if command_prompt.is_some()
        && !pending.is_empty()
        && pending_since
            .is_some_and(|since| now.duration_since(since) >= COPY_MODE_ESCAPE_DISAMBIGUATION)
    {
        *command_prompt = None;
        pending.clear();
        *pending_since = None;
        return true;
    }
    false
}

#[cfg(test)]
fn translate_attach_input(input: &[u8], saw_prefix: &mut bool) -> Vec<AttachInputAction> {
    let mut state = RawAttachInputState {
        saw_prefix: *saw_prefix,
        selecting_pane: false,
        command_prompt: None,
        mouse_pending: Vec::new(),
        prompt_pending_since: None,
    };
    let actions = translate_attach_input_with_state(input, &mut state);
    *saw_prefix = state.saw_prefix;
    actions
}

#[cfg(test)]
fn translate_attach_input_with_state(
    input: &[u8],
    state: &mut RawAttachInputState,
) -> Vec<AttachInputAction> {
    translate_attach_input_with_state_with_controls(input, state, &LiveControls::default())
}

fn translate_attach_input_with_state_with_controls(
    input: &[u8],
    state: &mut RawAttachInputState,
    controls: &LiveControls,
) -> Vec<AttachInputAction> {
    let mut bytes = Vec::new();
    if !state.mouse_pending.is_empty() {
        bytes.extend_from_slice(&state.mouse_pending);
        state.mouse_pending.clear();
        state.prompt_pending_since = None;
    }
    bytes.extend_from_slice(input);

    let mut output = Vec::with_capacity(bytes.len());
    let mut actions = Vec::new();
    let mut offset = 0;

    while offset < bytes.len() {
        if let Some(command) = state.command_prompt.as_mut() {
            if bytes[offset] == 0x1b {
                let remaining = &bytes[offset..];
                if let Some(consumed) = complete_sgr_mouse_event_len(remaining) {
                    state.command_prompt = None;
                    actions.push(AttachInputAction::CommandPromptCancel);
                    offset += consumed;
                    continue;
                }
                if let Some(consumed) = complete_csi_sequence_len(remaining) {
                    state.command_prompt = None;
                    actions.push(AttachInputAction::CommandPromptCancel);
                    offset += consumed;
                    continue;
                }
                if is_incomplete_prompt_escape_sequence(remaining) {
                    state.mouse_pending.extend_from_slice(remaining);
                    state.prompt_pending_since = Some(Instant::now());
                    break;
                }
            }

            let byte = bytes[offset];
            match byte {
                b'\r' | b'\n' => {
                    let command = String::from_utf8_lossy(command).trim().to_string();
                    state.command_prompt = None;
                    if command.is_empty() {
                        actions.push(AttachInputAction::CommandPromptCancel);
                    } else {
                        actions.push(AttachInputAction::CommandPromptDispatch {
                            command,
                            trailing_input: bytes[offset + 1..].to_vec(),
                        });
                        break;
                    }
                }
                0x03 | 0x1b => {
                    state.command_prompt = None;
                    actions.push(AttachInputAction::CommandPromptCancel);
                }
                0x7f | 0x08 => {
                    command.pop();
                    actions.push(AttachInputAction::CommandPromptUpdate(
                        String::from_utf8_lossy(command).to_string(),
                    ));
                }
                byte if !byte.is_ascii_control() => {
                    command.push(byte);
                    actions.push(AttachInputAction::CommandPromptUpdate(
                        String::from_utf8_lossy(command).to_string(),
                    ));
                }
                _ => {}
            }
            offset += 1;
            continue;
        }

        let byte = bytes[offset];
        if state.selecting_pane {
            state.selecting_pane = false;
            if byte.is_ascii_digit() {
                actions.push(AttachInputAction::SelectPane(usize::from(byte - b'0')));
                offset += 1;
                continue;
            }
            if byte == controls.prefix {
                state.saw_prefix = true;
                offset += 1;
                continue;
            }
            output.push(byte);
            offset += 1;
            continue;
        }

        if state.saw_prefix {
            match parse_input_key(&bytes[offset..]) {
                ParsedInputKey::Key(key, consumed) => {
                    state.saw_prefix = false;
                    if let Some(action) = controls.action_for_key(key) {
                        let flow = push_attach_key_action(
                            &mut actions,
                            &mut output,
                            state,
                            controls,
                            action,
                            &bytes[offset + consumed..],
                        );
                        offset += consumed;
                        if flow == KeyActionFlow::Stop {
                            return actions;
                        }
                        continue;
                    }
                    output.push(controls.prefix);
                    output.extend_from_slice(&bytes[offset..offset + consumed]);
                    offset += consumed;
                    continue;
                }
                ParsedInputKey::Unhandled(consumed) => {
                    state.saw_prefix = false;
                    output.push(controls.prefix);
                    output.extend_from_slice(&bytes[offset..offset + consumed]);
                    offset += consumed;
                    continue;
                }
                ParsedInputKey::Incomplete => {
                    if !output.is_empty() {
                        actions.push(AttachInputAction::Forward(std::mem::take(&mut output)));
                    }
                    state.mouse_pending.extend_from_slice(&bytes[offset..]);
                    break;
                }
            }
        }

        if byte == controls.prefix {
            state.saw_prefix = true;
        } else if let ParsedInputKey::Key(key, consumed) = parse_input_key(&bytes[offset..]) {
            if key.is_global_binding() {
                if let Some(action) = controls.action_for_key(key) {
                    let flow = push_attach_key_action(
                        &mut actions,
                        &mut output,
                        state,
                        controls,
                        action,
                        &bytes[offset + consumed..],
                    );
                    offset += consumed;
                    if flow == KeyActionFlow::Stop {
                        return actions;
                    }
                    continue;
                }
            }
            output.extend_from_slice(&bytes[offset..offset + consumed]);
            offset += consumed;
            continue;
        } else if let ParsedInputKey::Unhandled(consumed) = parse_input_key(&bytes[offset..]) {
            output.extend_from_slice(&bytes[offset..offset + consumed]);
            offset += consumed;
            continue;
        } else if matches!(
            parse_input_key(&bytes[offset..]),
            ParsedInputKey::Incomplete
        ) {
            if !output.is_empty() {
                actions.push(AttachInputAction::Forward(std::mem::take(&mut output)));
            }
            state.mouse_pending.extend_from_slice(&bytes[offset..]);
            break;
        } else {
            output.push(byte);
        }
        offset += 1;
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

#[cfg(test)]
fn translate_live_snapshot_input_with_mouse(
    input: &[u8],
    state: &mut LiveSnapshotInputState,
    mouse_focus_enabled: bool,
) -> Vec<LiveSnapshotInputAction> {
    translate_live_snapshot_input_with_mouse_and_controls(
        input,
        state,
        mouse_focus_enabled,
        &LiveControls::default(),
    )
}

fn translate_live_snapshot_input_with_mouse_and_controls(
    input: &[u8],
    state: &mut LiveSnapshotInputState,
    mouse_focus_enabled: bool,
    controls: &LiveControls,
) -> Vec<LiveSnapshotInputAction> {
    let mut bytes = Vec::new();
    if !state.mouse_pending.is_empty() {
        bytes.extend_from_slice(&state.mouse_pending);
        state.mouse_pending.clear();
        state.prompt_pending_since = None;
    }
    bytes.extend_from_slice(input);

    let mut output = Vec::with_capacity(bytes.len());
    let mut actions = Vec::new();
    let mut offset = 0;

    while offset < bytes.len() {
        if let Some(command) = state.command_prompt.as_mut() {
            if bytes[offset] == 0x1b {
                let remaining = &bytes[offset..];
                if let Some(consumed) = complete_sgr_mouse_event_len(remaining) {
                    state.command_prompt = None;
                    actions.push(LiveSnapshotInputAction::CommandPromptCancel);
                    offset += consumed;
                    continue;
                }
                if let Some(consumed) = complete_csi_sequence_len(remaining) {
                    state.command_prompt = None;
                    actions.push(LiveSnapshotInputAction::CommandPromptCancel);
                    offset += consumed;
                    continue;
                }
                if is_incomplete_prompt_escape_sequence(remaining) {
                    state.mouse_pending.extend_from_slice(remaining);
                    state.prompt_pending_since = Some(Instant::now());
                    break;
                }
            }
            let byte = bytes[offset];
            match byte {
                b'\r' | b'\n' => {
                    let command = String::from_utf8_lossy(command).trim().to_string();
                    state.command_prompt = None;
                    if command.is_empty() {
                        actions.push(LiveSnapshotInputAction::CommandPromptCancel);
                    } else {
                        actions.push(LiveSnapshotInputAction::CommandPromptDispatch {
                            command,
                            trailing_input: bytes[offset + 1..].to_vec(),
                        });
                        break;
                    }
                }
                0x03 | 0x1b => {
                    state.command_prompt = None;
                    actions.push(LiveSnapshotInputAction::CommandPromptCancel);
                }
                0x7f | 0x08 => {
                    command.pop();
                    actions.push(LiveSnapshotInputAction::CommandPromptUpdate(
                        String::from_utf8_lossy(command).to_string(),
                    ));
                }
                byte if !byte.is_ascii_control() => {
                    command.push(byte);
                    actions.push(LiveSnapshotInputAction::CommandPromptUpdate(
                        String::from_utf8_lossy(command).to_string(),
                    ));
                }
                _ => {}
            }
            offset += 1;
            continue;
        }

        if mouse_focus_enabled && bytes[offset] == 0x1b {
            let remaining = &bytes[offset..];
            if let Some((event, consumed)) = parse_sgr_mouse_event(remaining) {
                if let Some(position) = live_mouse_press_position(event) {
                    if !output.is_empty() {
                        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                            &mut output,
                        )));
                    }
                    clear_live_snapshot_command_state(state);
                    actions.push(LiveSnapshotInputAction::MousePress(position));
                } else {
                    output.extend_from_slice(&remaining[..consumed]);
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
                output.extend_from_slice(&remaining[..consumed]);
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
            if byte == controls.prefix {
                state.saw_prefix = true;
                offset += 1;
                continue;
            }
            output.push(byte);
            offset += 1;
            continue;
        }

        if state.saw_prefix {
            match parse_input_key(&bytes[offset..]) {
                ParsedInputKey::Key(key, consumed) => {
                    state.saw_prefix = false;
                    if let Some(action) = controls.action_for_key(key) {
                        let flow = push_live_snapshot_key_action(
                            &mut actions,
                            &mut output,
                            state,
                            controls,
                            action,
                            &bytes[offset + consumed..],
                        );
                        offset += consumed;
                        if flow == KeyActionFlow::Stop {
                            return actions;
                        }
                        continue;
                    }
                    output.push(controls.prefix);
                    output.extend_from_slice(&bytes[offset..offset + consumed]);
                    offset += consumed;
                    continue;
                }
                ParsedInputKey::Unhandled(consumed) => {
                    state.saw_prefix = false;
                    output.push(controls.prefix);
                    output.extend_from_slice(&bytes[offset..offset + consumed]);
                    offset += consumed;
                    continue;
                }
                ParsedInputKey::Incomplete => {
                    if !output.is_empty() {
                        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                            &mut output,
                        )));
                    }
                    state.mouse_pending.extend_from_slice(&bytes[offset..]);
                    break;
                }
            }
        }

        if byte == controls.prefix {
            state.saw_prefix = true;
        } else {
            match parse_input_key(&bytes[offset..]) {
                ParsedInputKey::Key(key, consumed) => {
                    if key.is_global_binding() {
                        if let Some(action) = controls.action_for_key(key) {
                            let flow = push_live_snapshot_key_action(
                                &mut actions,
                                &mut output,
                                state,
                                controls,
                                action,
                                &bytes[offset + consumed..],
                            );
                            offset += consumed;
                            if flow == KeyActionFlow::Stop {
                                return actions;
                            }
                            continue;
                        }
                    }
                    output.extend_from_slice(&bytes[offset..offset + consumed]);
                    offset += consumed;
                    continue;
                }
                ParsedInputKey::Unhandled(consumed) => {
                    output.extend_from_slice(&bytes[offset..offset + consumed]);
                    offset += consumed;
                    continue;
                }
                ParsedInputKey::Incomplete => {
                    if !output.is_empty() {
                        actions.push(LiveSnapshotInputAction::Forward(std::mem::take(
                            &mut output,
                        )));
                    }
                    state.mouse_pending.extend_from_slice(&bytes[offset..]);
                    break;
                }
            }
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
    state.command_prompt = None;
}

#[cfg(test)]
fn finish_live_snapshot_input(state: &mut LiveSnapshotInputState) -> Option<Vec<u8>> {
    finish_live_snapshot_input_with_controls(state, &LiveControls::default())
}

fn finish_live_snapshot_input_with_controls(
    state: &mut LiveSnapshotInputState,
    controls: &LiveControls,
) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    if state.saw_prefix {
        state.saw_prefix = false;
        output.push(controls.prefix);
    }
    state.selecting_pane = false;
    if state.command_prompt.is_some() {
        state.command_prompt = None;
        state.mouse_pending.clear();
        state.prompt_pending_since = None;
    } else if !state.mouse_pending.is_empty() {
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
    PaneCommand(PaneCommand),
    CommandPromptStart,
    CommandPromptUpdate(String),
    CommandPromptDispatch {
        command: String,
        dispatch_done: mpsc::Sender<()>,
    },
    CommandPromptCancel,
    SelectNextPane,
    ShowPaneNumbers,
    ShowHelp,
    SelectPane(usize),
    PauseRedraw(mpsc::Sender<()>),
    RedrawNow,
    RedrawHint,
    RenderFrame(AttachRenderFrame),
    RenderStreamReady,
    RenderStreamUnavailable,
    RenderStreamClosed,
    EventStreamReady,
    EventStreamClosed,
    Error(String),
    Detach,
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveSnapshotServerEvent {
    Redraw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveSnapshotRedrawHintStatus {
    Sent,
    Coalesced,
    Disconnected,
}

fn parse_live_snapshot_event_line(line: &str) -> io::Result<LiveSnapshotServerEvent> {
    match line {
        "REDRAW\n" => Ok(LiveSnapshotServerEvent::Redraw),
        _ => Err(io::Error::other("unknown attach event")),
    }
}

fn spawn_live_snapshot_input_thread(
    socket: PathBuf,
    session: String,
    mouse_focus_enabled: Arc<AtomicBool>,
    initial_input: Vec<u8>,
    sender: mpsc::Sender<LiveSnapshotInputEvent>,
) {
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0_u8; 1024];
        let mut input_state = LiveSnapshotInputState::default();
        let mut controls = load_live_controls(&socket);
        let mouse_enabled = mouse_focus_enabled.load(Ordering::SeqCst);
        let actions = translate_live_snapshot_input_with_mouse_and_controls(
            &initial_input,
            &mut input_state,
            mouse_enabled,
            &controls,
        );
        if !send_live_snapshot_input_actions(
            &socket,
            &session,
            &sender,
            &mut stdin,
            &mut input_state,
            &mouse_focus_enabled,
            &mut controls,
            actions,
        ) {
            return;
        }

        loop {
            if let Some(action) = input_state.prompt_pending_timeout_action(Instant::now()) {
                if !send_live_snapshot_input_actions(
                    &socket,
                    &session,
                    &sender,
                    &mut stdin,
                    &mut input_state,
                    &mouse_focus_enabled,
                    &mut controls,
                    vec![action],
                ) {
                    return;
                }
            }
            let read_result = if input_state.prompt_pending_since.is_some() && stdin_is_tty() {
                match NonBlockingFdGuard::enable(STDIN_FILENO) {
                    Ok(_guard) => stdin.read(&mut buf),
                    Err(error) => Err(error),
                }
            } else {
                stdin.read(&mut buf)
            };
            let n = match read_result {
                Ok(0) => {
                    if let Some(bytes) =
                        finish_live_snapshot_input_with_controls(&mut input_state, &controls)
                    {
                        let _ = sender.send(LiveSnapshotInputEvent::Forward(bytes));
                    }
                    let _ = sender.send(LiveSnapshotInputEvent::Eof);
                    break;
                }
                Ok(n) => n,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(_) => {
                    let _ = sender.send(LiveSnapshotInputEvent::Eof);
                    break;
                }
            };

            controls = load_live_controls(&socket);
            let mouse_enabled = mouse_focus_enabled.load(Ordering::SeqCst);
            let actions = translate_live_snapshot_input_with_mouse_and_controls(
                &buf[..n],
                &mut input_state,
                mouse_enabled,
                &controls,
            );
            if !send_live_snapshot_input_actions(
                &socket,
                &session,
                &sender,
                &mut stdin,
                &mut input_state,
                &mouse_focus_enabled,
                &mut controls,
                actions,
            ) {
                return;
            }
        }
    });
}

fn send_live_snapshot_input_actions<R: Read>(
    socket: &Path,
    session: &str,
    sender: &mpsc::Sender<LiveSnapshotInputEvent>,
    stdin: &mut R,
    input_state: &mut LiveSnapshotInputState,
    mouse_focus_enabled: &Arc<AtomicBool>,
    controls: &mut LiveControls,
    actions: Vec<LiveSnapshotInputAction>,
) -> bool {
    for action in actions {
        match action {
            LiveSnapshotInputAction::Forward(bytes) => {
                let _ = sender.send(LiveSnapshotInputEvent::Forward(bytes));
            }
            LiveSnapshotInputAction::PaneCommand(command) => {
                let _ = sender.send(LiveSnapshotInputEvent::PaneCommand(command));
            }
            LiveSnapshotInputAction::CommandPromptStart => {
                let _ = sender.send(LiveSnapshotInputEvent::CommandPromptStart);
            }
            LiveSnapshotInputAction::CommandPromptUpdate(command) => {
                let _ = sender.send(LiveSnapshotInputEvent::CommandPromptUpdate(command));
            }
            LiveSnapshotInputAction::CommandPromptDispatch {
                command,
                trailing_input,
            } => {
                let (dispatch_done, dispatch_wait) = mpsc::channel();
                if sender
                    .send(LiveSnapshotInputEvent::CommandPromptDispatch {
                        command,
                        dispatch_done,
                    })
                    .is_err()
                {
                    return false;
                }
                if dispatch_wait.recv().is_err() {
                    return false;
                }
                *controls = load_live_controls(socket);
                if !trailing_input.is_empty() {
                    let mouse_enabled = mouse_focus_enabled.load(Ordering::SeqCst);
                    let actions = translate_live_snapshot_input_with_mouse_and_controls(
                        &trailing_input,
                        input_state,
                        mouse_enabled,
                        controls,
                    );
                    if !send_live_snapshot_input_actions(
                        socket,
                        session,
                        sender,
                        stdin,
                        input_state,
                        mouse_focus_enabled,
                        controls,
                        actions,
                    ) {
                        return false;
                    }
                }
            }
            LiveSnapshotInputAction::CommandPromptCancel => {
                let _ = sender.send(LiveSnapshotInputEvent::CommandPromptCancel);
            }
            LiveSnapshotInputAction::Detach => {
                let _ = sender.send(LiveSnapshotInputEvent::Detach);
                return false;
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
                match run_composed_copy_mode_with_reader(socket, session, &initial_input, stdin) {
                    Ok(()) => {
                        let _ = sender.send(LiveSnapshotInputEvent::RedrawNow);
                    }
                    Err(error) => {
                        let _ = sender.send(LiveSnapshotInputEvent::Error(error.to_string()));
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn spawn_live_snapshot_event_thread(
    socket: PathBuf,
    session: String,
    sender: mpsc::Sender<LiveSnapshotInputEvent>,
    redraw_hint_pending: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let mut stream = match UnixStream::connect(socket) {
            Ok(stream) => stream,
            Err(_) => return,
        };
        if stream
            .write_all(protocol::encode_attach_events(&session).as_bytes())
            .is_err()
        {
            return;
        }

        let response = match read_line(&mut stream) {
            Ok(response) => response,
            Err(_) => return,
        };
        if response != "OK\n" {
            return;
        }
        if sender
            .send(LiveSnapshotInputEvent::EventStreamReady)
            .is_err()
        {
            return;
        }

        loop {
            let line = match read_line(&mut stream) {
                Ok(line) => line,
                Err(_) => {
                    let _ = sender.send(LiveSnapshotInputEvent::EventStreamClosed);
                    return;
                }
            };
            if line.is_empty() {
                let _ = sender.send(LiveSnapshotInputEvent::EventStreamClosed);
                return;
            }

            match parse_live_snapshot_event_line(&line) {
                Ok(LiveSnapshotServerEvent::Redraw) => {
                    if send_live_snapshot_redraw_hint(&sender, &redraw_hint_pending)
                        == LiveSnapshotRedrawHintStatus::Disconnected
                    {
                        return;
                    }
                }
                Err(_) => {}
            }
        }
    });
}

fn spawn_live_snapshot_render_thread(
    socket: PathBuf,
    session: String,
    sender: mpsc::Sender<LiveSnapshotInputEvent>,
) {
    std::thread::spawn(move || {
        let mut stream = match UnixStream::connect(socket) {
            Ok(stream) => stream,
            Err(_) => {
                let _ = sender.send(LiveSnapshotInputEvent::RenderStreamUnavailable);
                return;
            }
        };
        if stream
            .write_all(protocol::encode_attach_render(&session).as_bytes())
            .is_err()
        {
            let _ = sender.send(LiveSnapshotInputEvent::RenderStreamUnavailable);
            return;
        }

        let response = match read_line(&mut stream) {
            Ok(response) => response,
            Err(_) => {
                let _ = sender.send(LiveSnapshotInputEvent::RenderStreamUnavailable);
                return;
            }
        };
        if response != ATTACH_RENDER_RESPONSE {
            let _ = sender.send(LiveSnapshotInputEvent::RenderStreamUnavailable);
            return;
        }
        if sender
            .send(LiveSnapshotInputEvent::RenderStreamReady)
            .is_err()
        {
            return;
        }

        loop {
            match read_attach_render_frame(&mut stream) {
                Ok(frame) => {
                    if sender
                        .send(LiveSnapshotInputEvent::RenderFrame(frame))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(_) => {
                    let _ = sender.send(LiveSnapshotInputEvent::RenderStreamClosed);
                    return;
                }
            }
        }
    });
}

fn spawn_live_snapshot_lifetime_thread(
    mut stream: UnixStream,
    sender: mpsc::Sender<LiveSnapshotInputEvent>,
) {
    std::thread::spawn(move || {
        let mut buf = [0_u8; 1];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => {
                    let _ = sender.send(LiveSnapshotInputEvent::Eof);
                    return;
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = sender.send(LiveSnapshotInputEvent::Eof);
                    return;
                }
            }
        }
    });
}

fn send_live_snapshot_redraw_hint(
    sender: &mpsc::Sender<LiveSnapshotInputEvent>,
    pending: &AtomicBool,
) -> LiveSnapshotRedrawHintStatus {
    if pending
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return LiveSnapshotRedrawHintStatus::Coalesced;
    }

    if sender.send(LiveSnapshotInputEvent::RedrawHint).is_ok() {
        return LiveSnapshotRedrawHintStatus::Sent;
    }

    pending.store(false, Ordering::SeqCst);
    LiveSnapshotRedrawHintStatus::Disconnected
}

fn live_snapshot_redraw_timeout(
    event_stream_active: bool,
    redraw_paused: bool,
    message_expiry: Option<Instant>,
    input_repaint_deadline: Option<Instant>,
    now: Instant,
) -> Duration {
    let base = if event_stream_active {
        LIVE_SNAPSHOT_EVENT_SAFETY_REDRAW_INTERVAL
    } else {
        LIVE_SNAPSHOT_REDRAW_INTERVAL
    };

    if redraw_paused {
        return base;
    }

    let mut timeout = base;
    if let Some(expiry) = message_expiry {
        if expiry <= now {
            return Duration::ZERO;
        }
        timeout = timeout.min(expiry.duration_since(now));
    }
    if let Some(deadline) = input_repaint_deadline {
        if deadline <= now {
            return Duration::ZERO;
        }
        timeout = timeout.min(deadline.duration_since(now));
    }
    timeout
}

fn handle_render_stream_ready(_event_stream_active: &mut bool) {
    // The OK response proves the stream connected, but not that frames are flowing yet.
}

fn handle_render_frame_received(event_stream_active: &mut bool) {
    *event_stream_active = true;
}

fn run_live_snapshot_attach(
    socket: &Path,
    session: &str,
    stream: &mut UnixStream,
    initial_input: Vec<u8>,
    last_size: &mut Option<PtySize>,
    on_resize: &mut impl FnMut(PtySize) -> io::Result<()>,
) -> io::Result<()> {
    let mouse_focus_enabled = Arc::new(AtomicBool::new(false));
    let (sender, input) = mpsc::channel();
    spawn_live_snapshot_input_thread(
        socket.to_path_buf(),
        session.to_string(),
        Arc::clone(&mouse_focus_enabled),
        initial_input,
        sender.clone(),
    );
    let redraw_hint_pending = Arc::new(AtomicBool::new(false));
    spawn_live_snapshot_render_thread(socket.to_path_buf(), session.to_string(), sender.clone());
    let fallback_sender = sender.clone();
    spawn_live_snapshot_lifetime_thread(stream.try_clone()?, sender);
    let _screen_guard = AlternateScreenGuard::enter()?;
    let mut frame = write_initial_live_snapshot_frame(socket, session)?;
    let mut mouse_mode = None;
    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
    let mut last_redraw = Instant::now();
    let mut redraw_paused = false;
    let mut event_stream_active = false;
    let mut fallback_event_stream_started = false;
    let mut pane_number_message = None;
    let mut command_prompt_message: Option<String> = None;
    let mut help_popup_visible = false;
    let mut pending_input_repaint_deadline = None;
    let mut render_output_state = LiveRenderOutputState::default();

    loop {
        let message_expiry = pane_number_message.as_ref().map(|(_, until)| *until);
        let timeout = live_snapshot_redraw_timeout(
            event_stream_active,
            redraw_paused,
            message_expiry,
            pending_input_repaint_deadline,
            Instant::now(),
        );
        match input.recv_timeout(timeout) {
            Ok(LiveSnapshotInputEvent::Forward(bytes)) => {
                let resized = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                forward_live_snapshot_input(stream, &bytes)?;
                if !redraw_paused
                    && (resized
                        || (!event_stream_active
                            && last_redraw.elapsed() >= LIVE_SNAPSHOT_REDRAW_INTERVAL))
                {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                    last_redraw = Instant::now();
                    pending_input_repaint_deadline = None;
                } else if event_stream_active && !bytes.is_empty() {
                    pending_input_repaint_deadline =
                        Some(Instant::now() + LIVE_SNAPSHOT_EVENT_SAFETY_REDRAW_INTERVAL);
                }
            }
            Ok(LiveSnapshotInputEvent::PaneCommand(command)) => {
                let _ = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                let result = apply_live_pane_command(socket, session, command, &frame);
                pane_number_message = match result {
                    Ok(()) => None,
                    Err(error) => Some((
                        error.to_string(),
                        Instant::now() + PANE_NUMBER_DISPLAY_DURATION,
                    )),
                };
                if !redraw_paused {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                }
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::CommandPromptStart) => {
                help_popup_visible = false;
                pane_number_message = None;
                command_prompt_message = Some(attach_command_prompt_text(""));
                if !redraw_paused {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                }
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::CommandPromptUpdate(command)) => {
                help_popup_visible = false;
                pane_number_message = None;
                command_prompt_message = Some(attach_command_prompt_text(&command));
                if !redraw_paused {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                }
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::CommandPromptCancel) => {
                help_popup_visible = false;
                command_prompt_message = None;
                pane_number_message = Some((
                    "cancelled".to_string(),
                    Instant::now() + PANE_NUMBER_DISPLAY_DURATION,
                ));
                if !redraw_paused {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                }
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::CommandPromptDispatch {
                command,
                dispatch_done,
            }) => {
                let _ = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                help_popup_visible = false;
                command_prompt_message = None;
                let command_result = dispatch_attach_command(socket, session, &command);
                pane_number_message = match command_result {
                    Ok(AttachCommandResult {
                        exit_attach: true, ..
                    }) => {
                        let _ = dispatch_done.send(());
                        break;
                    }
                    Ok(AttachCommandResult {
                        message: Some(message),
                        ..
                    }) => Some((message, Instant::now() + PANE_NUMBER_DISPLAY_DURATION)),
                    Ok(AttachCommandResult { .. }) => None,
                    Err(error) => Some((
                        error.to_string(),
                        Instant::now() + PANE_NUMBER_DISPLAY_DURATION,
                    )),
                };
                let _ = dispatch_done.send(());
                if !redraw_paused {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                }
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::SelectNextPane) => {
                let _ = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                select_next_pane(socket, session)?;
                pane_number_message = None;
                if !redraw_paused {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                }
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::ShowPaneNumbers) => {
                let _ = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                let message = pane_number_message_text(socket, session)?;
                pane_number_message =
                    Some((message, Instant::now() + PANE_NUMBER_DISPLAY_DURATION));
                frame = write_live_snapshot_frame_with_active_message(
                    socket,
                    session,
                    &mut pane_number_message,
                    command_prompt_message.as_deref(),
                    help_popup_visible,
                )?;
                sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                reset_live_render_output_state(&mut render_output_state);
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::ShowHelp) => {
                let _ = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                help_popup_visible = !help_popup_visible;
                pane_number_message = None;
                command_prompt_message = None;
                frame = write_live_snapshot_frame_with_active_message(
                    socket,
                    session,
                    &mut pane_number_message,
                    command_prompt_message.as_deref(),
                    help_popup_visible,
                )?;
                sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                reset_live_render_output_state(&mut render_output_state);
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::SelectPane(index)) => {
                let _ = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                let _ = select_numbered_pane(socket, session, index)?;
                pane_number_message = None;
                if !redraw_paused {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                }
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::MousePress(position)) => {
                let resized = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                if resized && !redraw_paused {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                    last_redraw = Instant::now();
                }
                if let Some(pane) =
                    pane_at_mouse_position(&frame.regions, frame.header_rows, position)
                {
                    let _ = select_numbered_pane(socket, session, pane)?;
                    pane_number_message = None;
                    if !redraw_paused {
                        frame = write_live_snapshot_frame_with_active_message(
                            socket,
                            session,
                            &mut pane_number_message,
                            command_prompt_message.as_deref(),
                            help_popup_visible,
                        )?;
                        sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                        reset_live_render_output_state(&mut render_output_state);
                    }
                    last_redraw = Instant::now();
                }
            }
            Ok(LiveSnapshotInputEvent::PauseRedraw(pause_ack)) => {
                redraw_paused = true;
                let _ = pause_ack.send(());
            }
            Ok(LiveSnapshotInputEvent::RedrawNow) => {
                let _ = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                redraw_paused = false;
                frame = write_live_snapshot_frame_with_active_message(
                    socket,
                    session,
                    &mut pane_number_message,
                    command_prompt_message.as_deref(),
                    help_popup_visible,
                )?;
                sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                reset_live_render_output_state(&mut render_output_state);
                last_redraw = Instant::now();
            }
            Ok(LiveSnapshotInputEvent::RedrawHint) => {
                let _ = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                redraw_hint_pending.store(false, Ordering::SeqCst);
                if !redraw_paused {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                    last_redraw = Instant::now();
                    pending_input_repaint_deadline = None;
                }
            }
            Ok(LiveSnapshotInputEvent::RenderFrame(render_frame)) => {
                let resized = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                if resized {
                    reset_live_render_output_state(&mut render_output_state);
                }
                handle_render_frame_received(&mut event_stream_active);
                if !redraw_paused {
                    frame = LiveSnapshotFrame {
                        regions: render_frame.regions.clone(),
                        header_rows: render_frame.header_rows,
                    };
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    if command_prompt_message.is_some()
                        || pane_number_message
                            .as_ref()
                            .is_some_and(|(_, until)| Instant::now() < *until)
                    {
                        frame = write_live_snapshot_frame_with_active_message(
                            socket,
                            session,
                            &mut pane_number_message,
                            command_prompt_message.as_deref(),
                            help_popup_visible,
                        )?;
                        sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                        reset_live_render_output_state(&mut render_output_state);
                    } else {
                        if command_prompt_message.is_none() && !help_popup_visible {
                            pane_number_message = None;
                        }
                        let help_popup_message = help_popup_visible.then(attach_help_overlay_text);
                        write_live_render_output(
                            &render_frame,
                            &mut render_output_state,
                            help_popup_message.as_deref(),
                        )?;
                    }
                    last_redraw = Instant::now();
                    pending_input_repaint_deadline = None;
                }
            }
            Ok(LiveSnapshotInputEvent::RenderStreamReady) => {
                handle_render_stream_ready(&mut event_stream_active);
            }
            Ok(LiveSnapshotInputEvent::RenderStreamUnavailable) => {
                if !fallback_event_stream_started {
                    spawn_live_snapshot_event_thread(
                        socket.to_path_buf(),
                        session.to_string(),
                        fallback_sender.clone(),
                        Arc::clone(&redraw_hint_pending),
                    );
                    fallback_event_stream_started = true;
                }
            }
            Ok(LiveSnapshotInputEvent::RenderStreamClosed) => {
                event_stream_active = false;
                if !fallback_event_stream_started {
                    spawn_live_snapshot_event_thread(
                        socket.to_path_buf(),
                        session.to_string(),
                        fallback_sender.clone(),
                        Arc::clone(&redraw_hint_pending),
                    );
                    fallback_event_stream_started = true;
                }
            }
            Ok(LiveSnapshotInputEvent::EventStreamReady) => {
                event_stream_active = true;
            }
            Ok(LiveSnapshotInputEvent::EventStreamClosed) => {
                event_stream_active = false;
            }
            Ok(LiveSnapshotInputEvent::Error(message)) => return Err(io::Error::other(message)),
            Ok(LiveSnapshotInputEvent::Detach) | Ok(LiveSnapshotInputEvent::Eof) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let resized = maybe_handle_live_snapshot_resize(last_size, on_resize)?;
                let message_expired = pane_number_message
                    .as_ref()
                    .is_some_and(|(_, until)| Instant::now() >= *until);
                let input_repaint_due = pending_input_repaint_deadline
                    .is_some_and(|deadline| Instant::now() >= deadline);
                if !redraw_paused
                    && (!event_stream_active || resized || message_expired || input_repaint_due)
                {
                    frame = write_live_snapshot_frame_with_active_message(
                        socket,
                        session,
                        &mut pane_number_message,
                        command_prompt_message.as_deref(),
                        help_popup_visible,
                    )?;
                    sync_live_mouse_mode(&mouse_focus_enabled, &mut mouse_mode, &frame)?;
                    reset_live_render_output_state(&mut render_output_state);
                    last_redraw = Instant::now();
                    pending_input_repaint_deadline = None;
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

fn forward_live_snapshot_input(stream: &mut UnixStream, bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }

    stream.write_all(bytes)
}

fn write_live_snapshot_frame_with_active_message(
    socket: &Path,
    session: &str,
    pane_number_message: &mut Option<(String, Instant)>,
    command_prompt_message: Option<&str>,
    help_popup_visible: bool,
) -> io::Result<LiveSnapshotFrame> {
    let help_popup_message = help_popup_visible.then(attach_help_overlay_text);
    let message = active_live_message(pane_number_message, command_prompt_message, Instant::now());
    write_live_snapshot_frame_with_message_and_clear(
        socket,
        session,
        message,
        help_popup_message.as_deref(),
        false,
        command_prompt_message.is_some(),
    )
}

fn active_live_message<'a>(
    pane_number_message: &'a mut Option<(String, Instant)>,
    command_prompt_message: Option<&'a str>,
    now: Instant,
) -> Option<&'a str> {
    if pane_number_message
        .as_ref()
        .is_some_and(|(_, until)| now >= *until)
    {
        *pane_number_message = None;
    }

    command_prompt_message.or_else(|| {
        pane_number_message
            .as_ref()
            .map(|(message, _)| message.as_str())
    })
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

fn apply_live_pane_command(
    socket: &Path,
    session: &str,
    command: PaneCommand,
    _frame: &LiveSnapshotFrame,
) -> io::Result<()> {
    match command {
        PaneCommand::NewWindow => {
            let _ = send_control_request(socket, &protocol::encode_new_window(session, &[]))?;
            Ok(())
        }
        PaneCommand::NextWindow => {
            let _ = send_control_request(socket, &protocol::encode_next_window(session))?;
            Ok(())
        }
        PaneCommand::PreviousWindow => {
            let _ = send_control_request(socket, &protocol::encode_previous_window(session))?;
            Ok(())
        }
        PaneCommand::SplitRight => {
            split_pane(socket, session, protocol::SplitDirection::Horizontal)
        }
        PaneCommand::SplitDown => split_pane(socket, session, protocol::SplitDirection::Vertical),
        PaneCommand::FocusLeft
        | PaneCommand::FocusDown
        | PaneCommand::FocusUp
        | PaneCommand::FocusRight => select_directional_pane(socket, session, command),
        PaneCommand::Resize { direction, amount } => {
            resize_pane(socket, session, direction, amount)
        }
        PaneCommand::Close => {
            let _ = send_control_request(socket, &protocol::encode_kill_pane(session, None))?;
            Ok(())
        }
        PaneCommand::ToggleZoom => {
            let _ = send_control_request(socket, &protocol::encode_zoom_pane(session, None))?;
            Ok(())
        }
    }
}

fn split_pane(socket: &Path, session: &str, direction: protocol::SplitDirection) -> io::Result<()> {
    let _ = send_control_request(socket, &protocol::encode_split(session, direction, &[]))?;
    Ok(())
}

fn resize_pane(
    socket: &Path,
    session: &str,
    direction: protocol::PaneResizeDirection,
    amount: usize,
) -> io::Result<()> {
    let _ = send_control_request(
        socket,
        &protocol::encode_resize_pane(session, direction, amount),
    )?;
    Ok(())
}

fn select_directional_pane(socket: &Path, session: &str, command: PaneCommand) -> io::Result<()> {
    let direction = pane_command_direction(command)
        .ok_or_else(|| io::Error::other("not a directional pane command"))?;
    let _ = send_control_request(
        socket,
        &protocol::encode_select_pane_target(
            session,
            protocol::PaneSelectTarget::Direction(direction),
        ),
    )?;
    Ok(())
}

fn pane_command_direction(command: PaneCommand) -> Option<protocol::PaneDirection> {
    match command {
        PaneCommand::FocusLeft => Some(protocol::PaneDirection::Left),
        PaneCommand::FocusDown => Some(protocol::PaneDirection::Down),
        PaneCommand::FocusUp => Some(protocol::PaneDirection::Up),
        PaneCommand::FocusRight => Some(protocol::PaneDirection::Right),
        _ => None,
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum RawAttachExit {
    Detach,
    Reconnect { pending_input: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawPendingFocus {
    Drop,
    Preserve,
}

fn raw_pending_input(
    actions: &[AttachInputAction],
    saw_prefix: bool,
    focus: RawPendingFocus,
    controls: &LiveControls,
) -> Vec<u8> {
    let mut pending = Vec::new();
    for action in actions {
        match action {
            AttachInputAction::Forward(bytes) => pending.extend_from_slice(bytes),
            AttachInputAction::PaneCommand(PaneCommand::FocusLeft)
            | AttachInputAction::PaneCommand(PaneCommand::FocusDown)
            | AttachInputAction::PaneCommand(PaneCommand::FocusUp)
            | AttachInputAction::PaneCommand(PaneCommand::FocusRight)
                if focus == RawPendingFocus::Drop => {}
            AttachInputAction::PaneCommand(command) => {
                push_pending_bound_action(
                    &mut pending,
                    controls,
                    LiveKeyAction::PaneCommand(*command),
                );
            }
            AttachInputAction::EnterCopyMode { initial_input } => {
                if push_pending_bound_action(&mut pending, controls, LiveKeyAction::CopyMode) {
                    pending.extend_from_slice(initial_input);
                }
            }
            AttachInputAction::SelectNextPane => {
                push_pending_bound_action(&mut pending, controls, LiveKeyAction::SelectNextPane);
            }
            AttachInputAction::ShowPaneNumbers => {
                push_pending_bound_action(&mut pending, controls, LiveKeyAction::ShowPaneNumbers);
            }
            AttachInputAction::SelectPane(index) => {
                if *index < 10 {
                    pending.push(b'0' + *index as u8);
                }
            }
            AttachInputAction::ShowHelp => {
                push_pending_bound_action(&mut pending, controls, LiveKeyAction::ShowHelp);
            }
            AttachInputAction::Detach => {
                push_pending_bound_action(&mut pending, controls, LiveKeyAction::Detach);
            }
            AttachInputAction::CommandPromptStart => {
                push_pending_bound_action(&mut pending, controls, LiveKeyAction::CommandPrompt);
            }
            AttachInputAction::CommandPromptDispatch {
                command,
                trailing_input,
            } => {
                pending.extend_from_slice(command.as_bytes());
                pending.push(b'\n');
                pending.extend_from_slice(trailing_input);
            }
            AttachInputAction::CommandPromptUpdate(_) | AttachInputAction::CommandPromptCancel => {}
        }
    }
    if saw_prefix {
        pending.push(controls.prefix);
    }
    pending
}

fn push_pending_bound_action(
    pending: &mut Vec<u8>,
    controls: &LiveControls,
    action: LiveKeyAction,
) -> bool {
    let Some(key) = controls.key_for_action(action) else {
        return false;
    };
    if !key.is_global_binding() {
        pending.push(controls.prefix);
    }
    pending.extend(key_stroke_input_bytes(key));
    true
}

fn forward_stdin_until_detach<F, C>(
    stream: &mut UnixStream,
    socket: &Path,
    session: &str,
    initial_input: Vec<u8>,
    stop_requested: Arc<AtomicBool>,
    copy_mode_active: Arc<AtomicBool>,
    mut tick: F,
    mut enter_copy_mode: C,
) -> io::Result<RawAttachExit>
where
    F: FnMut() -> io::Result<()>,
    C: FnMut(&[u8]) -> io::Result<()>,
{
    let mut buf = [0_u8; 1024];
    let mut input_state = RawAttachInputState::default();
    let mut initial_input = Some(initial_input);
    let mut controls = load_live_controls(socket);
    let mut help_popup_visible = false;

    loop {
        tick()?;
        if initial_input.is_none() && stop_requested.load(Ordering::SeqCst) {
            return Ok(RawAttachExit::Reconnect {
                pending_input: raw_pending_input(
                    &[],
                    input_state.saw_prefix,
                    RawPendingFocus::Preserve,
                    &controls,
                ),
            });
        }
        if input_state
            .prompt_pending_timeout_action(Instant::now())
            .is_some()
        {
            write_attach_transient_message("cancelled")?;
        }
        let actions = if let Some(input) = initial_input.take() {
            translate_attach_input_with_state_with_controls(&input, &mut input_state, &controls)
        } else {
            let read_result = {
                let _nonblocking_stdin = NonBlockingFdGuard::enable(STDIN_FILENO)?;
                io::stdin().read(&mut buf)
            };
            let n = match read_result {
                Ok(0) => break,
                Ok(n) => n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => return Err(error),
            };
            controls = load_live_controls(socket);
            translate_attach_input_with_state_with_controls(&buf[..n], &mut input_state, &controls)
        };

        for (index, action) in actions.iter().enumerate() {
            match action {
                AttachInputAction::Forward(output) => stream.write_all(&output)?,
                AttachInputAction::CommandPromptStart => write_attach_command_prompt("", true)?,
                AttachInputAction::CommandPromptUpdate(command) => {
                    write_attach_command_prompt(command, false)?
                }
                AttachInputAction::CommandPromptCancel => {
                    write_attach_transient_message("cancelled")?
                }
                AttachInputAction::CommandPromptDispatch {
                    command,
                    trailing_input,
                } => match dispatch_attach_command(socket, session, command) {
                    Ok(result) => {
                        if let Some(message) = result.message {
                            write_attach_transient_message(&message)?;
                        }
                        if result.exit_attach {
                            return Ok(RawAttachExit::Detach);
                        }
                        controls = load_live_controls(socket);
                        if result.reconnect {
                            let mut pending_input = trailing_input.clone();
                            pending_input.extend(raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ));
                            return Ok(RawAttachExit::Reconnect { pending_input });
                        }
                        if !trailing_input.is_empty() {
                            initial_input = Some(trailing_input.clone());
                            break;
                        }
                    }
                    Err(error) => {
                        write_attach_transient_message(&error.to_string())?;
                        controls = load_live_controls(socket);
                        if !trailing_input.is_empty() {
                            initial_input = Some(trailing_input.clone());
                            break;
                        }
                    }
                },
                AttachInputAction::PaneCommand(PaneCommand::NewWindow) => {
                    if let Err(error) =
                        send_control_request(socket, &protocol::encode_new_window(session, &[]))
                    {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::PaneCommand(PaneCommand::NextWindow) => {
                    if let Err(error) =
                        send_control_request(socket, &protocol::encode_next_window(session))
                    {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::PaneCommand(PaneCommand::PreviousWindow) => {
                    if let Err(error) =
                        send_control_request(socket, &protocol::encode_previous_window(session))
                    {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::PaneCommand(PaneCommand::SplitRight) => {
                    if let Err(error) =
                        split_pane(socket, session, protocol::SplitDirection::Horizontal)
                    {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::PaneCommand(PaneCommand::SplitDown) => {
                    if let Err(error) =
                        split_pane(socket, session, protocol::SplitDirection::Vertical)
                    {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::PaneCommand(PaneCommand::Close) => {
                    if let Err(error) =
                        send_control_request(socket, &protocol::encode_kill_pane(session, None))
                    {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::PaneCommand(PaneCommand::ToggleZoom) => {
                    if let Err(error) =
                        send_control_request(socket, &protocol::encode_zoom_pane(session, None))
                    {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::PaneCommand(PaneCommand::Resize { direction, amount }) => {
                    if let Err(error) = resize_pane(socket, session, *direction, *amount) {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::PaneCommand(PaneCommand::FocusLeft)
                | AttachInputAction::PaneCommand(PaneCommand::FocusDown)
                | AttachInputAction::PaneCommand(PaneCommand::FocusUp)
                | AttachInputAction::PaneCommand(PaneCommand::FocusRight) => {
                    let AttachInputAction::PaneCommand(command) = action else {
                        unreachable!();
                    };
                    if let Err(error) = select_directional_pane(socket, session, *command) {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::SelectNextPane => {
                    if let Err(error) = select_next_pane(socket, session) {
                        write_attach_transient_message(&error.to_string())?;
                    } else {
                        return Ok(RawAttachExit::Reconnect {
                            pending_input: raw_pending_input(
                                &actions[index + 1..],
                                input_state.saw_prefix,
                                RawPendingFocus::Preserve,
                                &controls,
                            ),
                        });
                    }
                }
                AttachInputAction::ShowPaneNumbers => {
                    match pane_number_message_text(socket, session) {
                        Ok(message) => write_attach_transient_message(&message)?,
                        Err(error) => write_attach_transient_message(&error.to_string())?,
                    }
                }
                AttachInputAction::SelectPane(pane) => {
                    match select_numbered_pane(socket, session, *pane) {
                        Ok(true) => {
                            return Ok(RawAttachExit::Reconnect {
                                pending_input: raw_pending_input(
                                    &actions[index + 1..],
                                    input_state.saw_prefix,
                                    RawPendingFocus::Preserve,
                                    &controls,
                                ),
                            });
                        }
                        Ok(false) => {}
                        Err(error) => write_attach_transient_message(&error.to_string())?,
                    }
                }
                AttachInputAction::EnterCopyMode { initial_input } => {
                    help_popup_visible = false;
                    copy_mode_active.store(true, Ordering::SeqCst);
                    let result = enter_copy_mode(initial_input);
                    copy_mode_active.store(false, Ordering::SeqCst);
                    result?;
                }
                AttachInputAction::ShowHelp => {
                    help_popup_visible = !help_popup_visible;
                    if help_popup_visible {
                        write_attach_help_message()?;
                    } else {
                        let _ = write_live_snapshot_frame_with_message_and_clear(
                            socket, session, None, None, false, false,
                        )?;
                    }
                }
                AttachInputAction::Detach => {
                    if help_popup_visible {
                        let _ = write_live_snapshot_frame_with_message_and_clear(
                            socket, session, None, None, false, false,
                        )?;
                    }
                    return Ok(RawAttachExit::Detach);
                }
            }
        }
    }

    if input_state.saw_prefix {
        stream.write_all(&[0x02])?;
    }

    Ok(RawAttachExit::Detach)
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
        &protocol::encode_copy_mode(session, protocol::CaptureMode::All, None, None),
    )?;
    let output = String::from_utf8_lossy(&body);
    let mut view = CopyModeView::from_numbered_output(&output)?;

    run_copy_mode_view_with_reader(socket, session, initial_input, stdin, &mut view)
}

#[allow(dead_code)]
fn run_composed_copy_mode_with_reader<R: Read>(
    socket: &Path,
    session: &str,
    initial_input: &[u8],
    stdin: &mut R,
) -> io::Result<()> {
    let snapshot = read_attach_layout_snapshot(socket, session)?;
    let text = String::from_utf8_lossy(&snapshot.snapshot);
    let mut view = CopyModeView::from_plain_text(&text)?;

    run_copy_mode_view_with_reader(socket, session, initial_input, stdin, &mut view)
}

fn run_copy_mode_view_with_reader<R: Read>(
    socket: &Path,
    session: &str,
    initial_input: &[u8],
    stdin: &mut R,
    view: &mut CopyModeView,
) -> io::Result<()> {
    let _mouse = MouseModeGuard::enable()?;
    write_copy_mode_view(view)?;
    if view.is_empty() {
        write_copy_mode_message("empty")?;
        return Ok(());
    }

    let mut input_state = CopyModeInputState::default();
    if handle_copy_mode_input(socket, session, view, &mut input_state, initial_input)? {
        return Ok(());
    }

    let _nonblocking_stdin = if stdin_is_tty() {
        Some(NonBlockingFdGuard::enable(STDIN_FILENO)?)
    } else {
        None
    };
    let mut buf = [0_u8; 1024];
    loop {
        if let Some(action) = input_state.pending_timeout_action(Instant::now()) {
            if apply_copy_mode_action(socket, session, view, action)? {
                break;
            }
        }

        let n = match stdin.read(&mut buf) {
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error),
        };
        if n == 0 {
            break;
        }

        if handle_copy_mode_input(socket, session, view, &mut input_state, &buf[..n])? {
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
            save_copy_mode_range(socket, session, view, line, line)?;
            Ok(true)
        }
        CopyModeAction::CopyLineRange { start, end } => {
            save_copy_mode_range(socket, session, view, start, end)?;
            Ok(true)
        }
        CopyModeAction::Exit => {
            write_copy_mode_message("exit")?;
            Ok(true)
        }
        CopyModeAction::Ignore => Ok(false),
    }
}

fn save_copy_mode_range(
    socket: &Path,
    session: &str,
    view: &CopyModeView,
    start: usize,
    end: usize,
) -> io::Result<()> {
    let request = encode_copy_mode_save_request(session, view, start, end)?;
    let body = send_control_request(socket, &request).map_err(map_copy_mode_save_text_error)?;
    let saved = String::from_utf8_lossy(&body);
    let saved = saved.trim_end();
    if saved.is_empty() {
        write_copy_mode_message("copied")?;
    } else {
        write_copy_mode_message(&format!("copied to {saved}"))?;
    }
    Ok(())
}

fn encode_copy_mode_save_request(
    session: &str,
    view: &CopyModeView,
    start: usize,
    end: usize,
) -> io::Result<String> {
    let text = view.selected_text_for_line_range(start, end)?;
    Ok(protocol::encode_save_buffer_text(session, None, &text))
}

fn map_copy_mode_save_text_error(error: io::Error) -> io::Error {
    if is_unknown_request_error(&error) {
        io::Error::other("copy-mode text save requires an updated dmux server")
    } else {
        error
    }
}

fn write_copy_mode_view(view: &mut CopyModeView) -> io::Result<()> {
    if let Some(size) = detect_attach_size() {
        view.set_height(usize::from(size.rows));
    }
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
    cli::attach_help_overlay()
}

fn attach_help_popup_content() -> Vec<String> {
    let mut saw_close_hint = false;
    let mut content = Vec::new();
    for line in attach_help_message().lines().map(str::trim_end) {
        if line.is_empty()
            || line.starts_with("Prompt accepts ")
            || line.starts_with("Key bindings and options ")
            || line.starts_with("Use list-keys/")
        {
            continue;
        }
        let line = line.replace("C-b ? toggle help", "C-b ? close");
        saw_close_hint |= line.contains("C-b ? close");
        content.push(line);
    }
    if !saw_close_hint {
        content.insert(0, "C-b ? close".to_string());
    }
    content
}

fn attach_help_overlay_text() -> String {
    attach_help_popup_content().join("\n")
}

#[cfg(test)]
fn attach_help_popup_message() -> String {
    boxed_popup_lines("dmux help", &attach_help_popup_content(), None, None).join("\n")
}

fn cell_padding(line: &str, target_width: usize) -> String {
    " ".repeat(target_width.saturating_sub(display_cell_width(line)))
}

fn write_attach_help_message() -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    let size = detect_attach_size().unwrap_or_else(default_attach_size);
    let mut output = Vec::new();
    append_centered_popup_overlay(
        &mut output,
        "dmux help",
        &attach_help_popup_content(),
        size,
        0,
    );
    stdout.write_all(&output)?;
    stdout.flush()
}

fn attach_command_prompt_text(command: &str) -> String {
    format!(
        ":{command}  (Enter run | Esc/C-c cancel | Backspace edit | examples: split -h; layout tiled; source-file path)"
    )
}

fn attach_command_prompt_display_text(command: &str, width: Option<usize>) -> String {
    truncate_header_line(&attach_command_prompt_text(command), width)
}

fn write_attach_command_prompt(command: &str, fresh: bool) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    let width = detect_attach_size().map(|size| usize::from(size.cols));
    if fresh {
        stdout.write_all(b"\r\n")?;
    } else {
        stdout.write_all(b"\r")?;
    }
    stdout.write_all(CLEAR_LINE)?;
    stdout.write_all(attach_command_prompt_display_text(command, width).as_bytes())?;
    stdout.flush()
}

fn write_attach_transient_message(message: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(format!("\r\n-- dmux: {message} --\r\n").as_bytes())?;
    stdout.flush()
}

struct AttachCommandResult {
    message: Option<String>,
    reconnect: bool,
    exit_attach: bool,
}

#[derive(Clone, Copy, Default)]
struct AttachCommandContext {
    source_depth: usize,
}

fn dispatch_attach_command(
    socket: &Path,
    session: &str,
    command: &str,
) -> io::Result<AttachCommandResult> {
    dispatch_attach_command_with_context(socket, session, command, AttachCommandContext::default())
}

fn dispatch_attach_command_with_context(
    socket: &Path,
    session: &str,
    command: &str,
    context: AttachCommandContext,
) -> io::Result<AttachCommandResult> {
    let sequence = crate::cli::parse_command_sequence(command).map_err(io::Error::other)?;
    if sequence.len() > 1 {
        return dispatch_attach_command_sequence(socket, session, &sequence, context);
    }

    let Some(command) = sequence.first() else {
        return Ok(AttachCommandResult {
            message: None,
            reconnect: false,
            exit_attach: false,
        });
    };
    let parts = command.argv.iter().map(String::as_str).collect::<Vec<_>>();
    let Some((name, args)) = parts.split_first() else {
        return Ok(AttachCommandResult {
            message: None,
            reconnect: false,
            exit_attach: false,
        });
    };

    let (body, reconnect, exit_attach) = match *name {
        "source-file" | "source" => {
            if context.source_depth > 0 {
                return Err(io::Error::other(
                    "nested source-file is not allowed in attach scripts",
                ));
            }
            let [path] = args else {
                return Err(io::Error::other("source-file requires exactly one path"));
            };
            return dispatch_attach_source_file(socket, session, path, context);
        }
        "rename-window" | "renamew" => {
            let name = args.join(" ");
            if name.is_empty() {
                return Err(io::Error::other("rename-window requires a name"));
            }
            (
                send_control_request(
                    socket,
                    &protocol::encode_rename_window(session, protocol::WindowTarget::Active, &name),
                )?,
                false,
                false,
            )
        }
        "rename-session" | "rename" => {
            let new_name = args.join(" ");
            if new_name.is_empty() {
                return Err(io::Error::other("rename-session requires a name"));
            }
            (
                send_control_request(socket, &protocol::encode_rename_session(session, &new_name))?,
                false,
                false,
            )
        }
        "select-window" | "selectw" => {
            let target = parse_attach_window_target(args)?;
            (
                send_control_request(
                    socket,
                    &protocol::encode_select_window_target(session, target),
                )?,
                true,
                false,
            )
        }
        "kill-window" | "killw" => {
            let target = if args.is_empty() {
                protocol::WindowTarget::Active
            } else {
                parse_attach_window_target(args)?
            };
            (
                send_control_request(
                    socket,
                    &protocol::encode_kill_window_target(session, target),
                )?,
                true,
                false,
            )
        }
        "kill-session" | "kill" => (
            send_control_request(socket, &protocol::encode_kill(session))?,
            false,
            true,
        ),
        "paste-buffer" | "pasteb" | "paste" => {
            let buffer = args.first().copied();
            (
                send_control_request(socket, &protocol::encode_paste_buffer(session, buffer))?,
                false,
                false,
            )
        }
        "list-windows" | "lsw" => (
            send_control_request(socket, &protocol::encode_list_windows(session, None))?,
            false,
            false,
        ),
        "list-buffers" | "lsb" => (
            send_control_request(socket, &protocol::encode_list_buffers(None))?,
            false,
            false,
        ),
        "list-keys" => (
            send_control_request(
                socket,
                &protocol::encode_list_keys(
                    parse_attach_optional_format(args, "list-keys")?.as_deref(),
                ),
            )?,
            false,
            false,
        ),
        "bind-key" => {
            let [key, command @ ..] = args else {
                return Err(io::Error::other("bind-key requires a key and command"));
            };
            let key = crate::config::canonical_key(key).map_err(io::Error::other)?;
            let command = crate::config::validate_binding_command(&command.join(" "))
                .map_err(io::Error::other)?;
            (
                send_control_request(socket, &protocol::encode_bind_key(&key, &command))?,
                false,
                false,
            )
        }
        "unbind-key" => {
            let [key] = args else {
                return Err(io::Error::other("unbind-key accepts exactly one key"));
            };
            let key = crate::config::canonical_key(key).map_err(io::Error::other)?;
            (
                send_control_request(socket, &protocol::encode_unbind_key(&key))?,
                false,
                false,
            )
        }
        "show-options" => (
            send_control_request(
                socket,
                &protocol::encode_show_options(
                    parse_attach_optional_format(args, "show-options")?.as_deref(),
                ),
            )?,
            false,
            false,
        ),
        "set-option" | "set" => {
            let [name, value] = args else {
                return Err(io::Error::other(
                    "set-option requires an option name and value",
                ));
            };
            let value =
                crate::config::validate_option_value(name, value).map_err(io::Error::other)?;
            (
                send_control_request(socket, &protocol::encode_set_option(name, &value))?,
                false,
                false,
            )
        }
        "split-window" | "split" => {
            let direction = if args.contains(&"-v") {
                protocol::SplitDirection::Vertical
            } else {
                protocol::SplitDirection::Horizontal
            };
            (
                send_control_request(socket, &protocol::encode_split(session, direction, &[]))?,
                true,
                false,
            )
        }
        "swap-pane" | "swapp" => {
            let (source, destination) = parse_attach_swap_pane_targets(session, args)?;
            (
                send_control_request(socket, &protocol::encode_swap_pane(&source, &destination))?,
                true,
                false,
            )
        }
        "move-pane" | "movep" => {
            let (source, destination, direction) =
                parse_attach_pane_transfer(session, args, "move-pane", true)?;
            (
                send_control_request(
                    socket,
                    &protocol::encode_move_pane(&source, &destination, direction),
                )?,
                true,
                false,
            )
        }
        "break-pane" | "breakp" => {
            let target = parse_attach_break_pane_target(session, args)?;
            (
                send_control_request(socket, &protocol::encode_break_pane(&target))?,
                true,
                false,
            )
        }
        "join-pane" | "joinp" => {
            let (source, destination, direction) =
                parse_attach_pane_transfer(session, args, "join-pane", false)?;
            (
                send_control_request(
                    socket,
                    &protocol::encode_join_pane(&source, &destination, direction),
                )?,
                true,
                false,
            )
        }
        "select-layout" | "layout" => {
            let preset = match args {
                [preset] => protocol::parse_layout_preset_name(preset).map_err(io::Error::other)?,
                [] => {
                    return Err(io::Error::other(
                        "select-layout requires a preset: even-horizontal, even-vertical, tiled, main-horizontal, or main-vertical",
                    ));
                }
                _ => {
                    return Err(io::Error::other("select-layout accepts exactly one preset"));
                }
            };
            (
                send_control_request(socket, &protocol::encode_select_layout(session, preset))?,
                true,
                false,
            )
        }
        other => {
            return Err(io::Error::other(format!(
                "unknown attach command {other:?}; press C-b ? for help or try :split -h, :swap-pane 1, :break-pane, :layout tiled, :list-windows; CLI automation is available with dmux run and dmux source-file"
            )));
        }
    };
    let message = String::from_utf8_lossy(&body).trim_end().to_string();
    Ok(AttachCommandResult {
        message: (!message.is_empty()).then_some(message),
        reconnect,
        exit_attach,
    })
}

fn dispatch_attach_command_sequence(
    socket: &Path,
    session: &str,
    commands: &[crate::cli::ScriptCommand],
    context: AttachCommandContext,
) -> io::Result<AttachCommandResult> {
    let mut reconnect = false;
    let mut message = None;

    for (index, command) in commands.iter().enumerate() {
        let result =
            dispatch_attach_command_with_context(socket, session, &command.source, context)
                .map_err(|err| {
                    io::Error::other(format!(
                        "command {} ({}) failed: {err}",
                        index + 1,
                        command.source
                    ))
                })?;
        reconnect |= result.reconnect;
        if result.message.is_some() {
            message = result.message;
        }
        if result.exit_attach {
            return Ok(AttachCommandResult {
                message,
                reconnect,
                exit_attach: true,
            });
        }
    }

    Ok(AttachCommandResult {
        message,
        reconnect,
        exit_attach: false,
    })
}

fn dispatch_attach_source_file(
    socket: &Path,
    session: &str,
    path: &str,
    context: AttachCommandContext,
) -> io::Result<AttachCommandResult> {
    let contents = std::fs::read_to_string(path)
        .map_err(|err| io::Error::other(format!("source-file {path:?}: {err}")))?;
    let commands = crate::cli::parse_command_file(&contents)
        .map_err(|err| io::Error::other(format!("source-file {path:?}: {err}")))?;

    let mut reconnect = false;
    let mut message = None;
    let nested_context = AttachCommandContext {
        source_depth: context.source_depth.saturating_add(1),
    };
    for entry in commands {
        let result = dispatch_attach_command_with_context(
            socket,
            session,
            &entry.command.source,
            nested_context,
        )
        .map_err(|err| {
            io::Error::other(format!(
                "source-file {path:?} line {} ({}) failed: {err}",
                entry.line, entry.command.source
            ))
        })?;
        reconnect |= result.reconnect;
        if result.message.is_some() {
            message = result.message;
        }
        if result.exit_attach {
            return Ok(AttachCommandResult {
                message,
                reconnect,
                exit_attach: true,
            });
        }
    }

    Ok(AttachCommandResult {
        message,
        reconnect,
        exit_attach: false,
    })
}

fn parse_attach_swap_pane_targets(
    session: &str,
    args: &[&str],
) -> io::Result<(protocol::Target, protocol::Target)> {
    if let [pane] = args {
        return Ok((
            protocol::Target::active(session.to_string()),
            parse_attach_pane_target(session, pane, "swap-pane")?,
        ));
    }

    let mut source = None;
    let mut destination = None;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-s" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| io::Error::other("swap-pane requires a source after -s"))?;
                source = Some(parse_attach_pane_target(session, value, "swap-pane")?);
                i += 2;
            }
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| io::Error::other("swap-pane requires a destination after -t"))?;
                destination = Some(parse_attach_pane_target(session, value, "swap-pane")?);
                i += 2;
            }
            other => {
                return Err(io::Error::other(format!(
                    "swap-pane does not support argument {other:?}; try :swap-pane 1"
                )));
            }
        }
    }

    Ok((
        source.ok_or_else(|| io::Error::other("swap-pane requires -s <pane>"))?,
        destination.ok_or_else(|| io::Error::other("swap-pane requires -t <pane>"))?,
    ))
}

fn parse_attach_break_pane_target(session: &str, args: &[&str]) -> io::Result<protocol::Target> {
    match args {
        [] => Ok(protocol::Target::active(session.to_string())),
        [pane] => parse_attach_pane_target(session, pane, "break-pane"),
        _ => Err(io::Error::other(
            "break-pane accepts at most one pane target; try :break-pane or :break-pane 1",
        )),
    }
}

fn parse_attach_pane_transfer(
    session: &str,
    args: &[&str],
    command: &str,
    destination_required: bool,
) -> io::Result<(protocol::Target, protocol::Target, protocol::SplitDirection)> {
    let mut source = None;
    let mut destination = None;
    let mut direction = protocol::SplitDirection::Horizontal;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-s" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    io::Error::other(format!("{command} requires a source after -s"))
                })?;
                source = Some(parse_attach_window_pane_target(session, value, command)?);
                i += 2;
            }
            "-t" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    io::Error::other(format!("{command} requires a destination after -t"))
                })?;
                destination = Some(parse_attach_window_pane_target(session, value, command)?);
                i += 2;
            }
            "-h" => {
                direction = protocol::SplitDirection::Horizontal;
                i += 1;
            }
            "-v" => {
                direction = protocol::SplitDirection::Vertical;
                i += 1;
            }
            other => {
                return Err(io::Error::other(format!(
                    "{command} does not support argument {other:?}"
                )));
            }
        }
    }

    let source = source.unwrap_or_else(|| protocol::Target::active(session.to_string()));
    let destination = if destination_required {
        destination
            .ok_or_else(|| io::Error::other(format!("{command} requires -t <window[.pane]>")))?
    } else {
        destination.unwrap_or_else(|| protocol::Target::active(session.to_string()))
    };
    Ok((source, destination, direction))
}

fn parse_attach_optional_format(args: &[&str], command_name: &str) -> io::Result<Option<String>> {
    let mut format = None;
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-F" | "--format" => {
                let value = args.get(index + 1).ok_or_else(|| {
                    io::Error::other(format!(
                        "{command_name} requires a format after {}",
                        args[index]
                    ))
                })?;
                format = Some((*value).to_string());
                index += 2;
            }
            value => {
                return Err(io::Error::other(format!(
                    "{command_name} does not support argument {value:?}"
                )));
            }
        }
    }
    Ok(format)
}

fn parse_attach_pane_target(
    session: &str,
    value: &str,
    command: &str,
) -> io::Result<protocol::Target> {
    Ok(protocol::Target {
        session: session.to_string(),
        window: protocol::WindowTarget::Active,
        pane: parse_attach_pane_token(value, command)?,
    })
}

fn parse_attach_window_pane_target(
    session: &str,
    value: &str,
    command: &str,
) -> io::Result<protocol::Target> {
    let (window, pane) = value
        .split_once('.')
        .map_or((value, None), |(window, pane)| (window, Some(pane)));
    Ok(protocol::Target {
        session: session.to_string(),
        window: parse_attach_window_token(window)?,
        pane: match pane {
            Some(pane) => parse_attach_pane_token(pane, command)?,
            None => protocol::PaneTarget::Active,
        },
    })
}

fn parse_attach_pane_token(value: &str, command: &str) -> io::Result<protocol::PaneTarget> {
    if let Some(id) = value.strip_prefix('%') {
        return id
            .parse::<usize>()
            .map(protocol::PaneTarget::Id)
            .map_err(|_| io::Error::other(format!("{command} has invalid pane id")));
    }
    value
        .parse::<usize>()
        .map(protocol::PaneTarget::Index)
        .map_err(|_| io::Error::other(format!("{command} has invalid pane index")))
}

fn parse_attach_window_token(value: &str) -> io::Result<protocol::WindowTarget> {
    if value.is_empty() {
        return Ok(protocol::WindowTarget::Active);
    }
    if let Some(id) = value.strip_prefix('@') {
        return id
            .parse::<usize>()
            .map(protocol::WindowTarget::Id)
            .map_err(|_| io::Error::other("invalid window id"));
    }
    if let Some(name) = value.strip_prefix('=') {
        return Ok(protocol::WindowTarget::Name(name.to_string()));
    }
    if let Ok(index) = value.parse::<usize>() {
        return Ok(protocol::WindowTarget::Index(index));
    }
    Ok(protocol::WindowTarget::Name(value.to_string()))
}

fn parse_attach_window_target(args: &[&str]) -> io::Result<protocol::WindowTarget> {
    let value = args
        .first()
        .ok_or_else(|| io::Error::other("select-window requires a target"))?;
    parse_attach_window_token(value)
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

fn load_live_controls(socket: &Path) -> LiveControls {
    let prefix = send_control_request(socket, &protocol::encode_show_options(None))
        .ok()
        .and_then(|body| {
            String::from_utf8(body).ok().and_then(|text| {
                text.lines().find_map(|line| {
                    let (name, value) = line.split_once('\t')?;
                    (name == crate::config::OPTION_PREFIX).then_some(value.to_string())
                })
            })
        })
        .unwrap_or_else(|| crate::config::DEFAULT_PREFIX_KEY.to_string());
    let bindings = send_control_request(socket, &protocol::encode_list_keys(None))
        .ok()
        .and_then(|body| {
            String::from_utf8(body).ok().map(|text| {
                text.lines()
                    .filter_map(|line| {
                        let (key, command) = line.split_once('\t')?;
                        Some(protocol::KeyBinding {
                            key: key.to_string(),
                            command: command.to_string(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_else(crate::config::default_key_bindings);
    LiveControls::from_entries(&prefix, bindings)
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
    pending_since: Option<Instant>,
}

impl CopyModeInputState {
    fn apply(&mut self, view: &mut CopyModeView, input: &[u8]) -> Vec<CopyModeAction> {
        let mut bytes = Vec::new();
        if !self.pending.is_empty() {
            bytes.extend_from_slice(&self.pending);
            self.pending.clear();
            self.pending_since = None;
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
                } else if let Some((key, consumed)) = parse_copy_mode_csi_key(remaining) {
                    offset += consumed;
                    actions.push(view.apply_key(key));
                } else if is_incomplete_copy_mode_csi_key(remaining) {
                    self.pending.extend_from_slice(remaining);
                    self.pending_since = Some(Instant::now());
                    break;
                } else if is_incomplete_sgr_mouse_event(remaining) {
                    self.pending.extend_from_slice(remaining);
                    self.pending_since = Some(Instant::now());
                    break;
                } else if let Some(consumed) = complete_csi_sequence_len(remaining) {
                    offset += consumed;
                    actions.push(CopyModeAction::Ignore);
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

    fn pending_timeout_action(&mut self, now: Instant) -> Option<CopyModeAction> {
        if self.pending.as_slice() == [0x1b]
            && self
                .pending_since
                .is_some_and(|since| now.duration_since(since) >= COPY_MODE_ESCAPE_DISAMBIGUATION)
        {
            self.pending.clear();
            self.pending_since = None;
            return Some(CopyModeAction::Exit);
        }
        None
    }
}

fn parse_copy_mode_csi_key(input: &[u8]) -> Option<(u8, usize)> {
    match input {
        [0x1b, b'[', b'A', ..] => Some((b'k', 3)),
        [0x1b, b'[', b'B', ..] => Some((b'j', 3)),
        [0x1b, b'[', b'H', ..] => Some((b'g', 3)),
        [0x1b, b'[', b'F', ..] => Some((b'G', 3)),
        [0x1b, b'[', b'5', b'~', ..] => Some((0x02, 4)),
        [0x1b, b'[', b'6', b'~', ..] => Some((0x06, 4)),
        _ => None,
    }
}

fn is_incomplete_copy_mode_csi_key(input: &[u8]) -> bool {
    input == [0x1b] || is_incomplete_csi_sequence(input)
}

fn is_incomplete_prompt_escape_sequence(input: &[u8]) -> bool {
    input == [0x1b] || is_incomplete_sgr_mouse_event(input) || is_incomplete_csi_sequence(input)
}

fn is_incomplete_csi_sequence(input: &[u8]) -> bool {
    input.starts_with(b"\x1b[") && input.len() < 64 && complete_csi_sequence_len(input).is_none()
}

fn complete_csi_sequence_len(input: &[u8]) -> Option<usize> {
    if !input.starts_with(b"\x1b[") {
        return None;
    }

    input[2..]
        .iter()
        .position(|byte| (0x40..=0x7e).contains(byte))
        .map(|end| 2 + end + 1)
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
    viewport: usize,
    height: usize,
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
            viewport: 0,
            height: 24,
            selection_anchor: None,
        })
    }

    fn from_plain_text(text: &str) -> io::Result<Self> {
        let normalized = text.replace("\r\n", "\n");
        let lines = normalized
            .split_terminator('\n')
            .enumerate()
            .map(|(index, text)| CopyModeLine {
                number: index + 1,
                text: text.to_string(),
            })
            .collect();

        Ok(Self {
            lines,
            cursor: 0,
            viewport: 0,
            height: 24,
            selection_anchor: None,
        })
    }

    fn cursor_line_number(&self) -> Option<usize> {
        self.lines.get(self.cursor).map(|line| line.number)
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn set_height(&mut self, height: usize) {
        self.height = height.max(2);
        self.ensure_cursor_visible();
    }

    #[cfg(test)]
    fn set_height_for_test(&mut self, height: usize) {
        self.set_height(height);
    }

    fn visible_line_count(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    fn ensure_cursor_visible(&mut self) {
        let visible = self.visible_line_count();
        if self.cursor < self.viewport {
            self.viewport = self.cursor;
        } else if self.cursor >= self.viewport + visible {
            self.viewport = self.cursor + 1 - visible;
        }
        self.clamp_viewport();
    }

    fn clamp_viewport(&mut self) {
        let visible = self.visible_line_count();
        self.viewport = self.viewport.min(self.lines.len().saturating_sub(visible));
    }

    fn move_cursor_to(&mut self, cursor: usize) -> CopyModeAction {
        if self.lines.is_empty() {
            return CopyModeAction::Ignore;
        }
        let cursor = cursor.min(self.lines.len() - 1);
        if cursor == self.cursor {
            return CopyModeAction::Ignore;
        }
        self.cursor = cursor;
        self.ensure_cursor_visible();
        CopyModeAction::Redraw
    }

    fn page_amount(&self) -> usize {
        self.visible_line_count().max(1)
    }

    fn selected_text_for_line_range(&self, start: usize, end: usize) -> io::Result<String> {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let mut selected = String::new();
        for line in &self.lines {
            if line.number >= start && line.number <= end {
                selected.push_str(&line.text);
                selected.push('\n');
            }
        }
        if selected.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "copy-mode line range is empty",
            ));
        }
        Ok(selected)
    }

    fn apply_key(&mut self, byte: u8) -> CopyModeAction {
        match byte {
            b'j' | 0x0e => self.move_cursor_to(self.cursor + 1),
            b'k' | 0x10 => self.move_cursor_to(self.cursor.saturating_sub(1)),
            0x06 => self.move_cursor_to(self.cursor.saturating_add(self.page_amount())),
            0x02 => self.move_cursor_to(self.cursor.saturating_sub(self.page_amount())),
            b'g' => self.move_cursor_to(0),
            b'G' => self.move_cursor_to(self.lines.len().saturating_sub(1)),
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
            self.ensure_cursor_visible();
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
                self.ensure_cursor_visible();
                return CopyModeAction::Redraw;
            }
            return CopyModeAction::Ignore;
        }

        if event.code & 3 == 0 {
            self.selection_anchor = Some(index);
            self.cursor = index;
            self.ensure_cursor_visible();
            return CopyModeAction::Redraw;
        }

        CopyModeAction::Ignore
    }

    fn line_index_for_mouse_row(&self, row: u16) -> Option<usize> {
        if row < 2 {
            return None;
        }
        let index = self.viewport + usize::from(row) - 2;
        self.lines.get(index).map(|_| index)
    }

    fn selected_bounds(&self) -> Option<(usize, usize)> {
        self.selection_anchor
            .map(|anchor| normalized_indexes(anchor, self.cursor))
    }

    fn render(&self) -> String {
        let mut output = String::from("\x1b[2J\x1b[H-- copy mode --\r\n");
        let selected = self.selected_bounds();
        let end = (self.viewport + self.visible_line_count()).min(self.lines.len());
        for (index, line) in self.lines[self.viewport..end].iter().enumerate() {
            let index = self.viewport + index;
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

struct NonBlockingFdGuard {
    fd: c_int,
    saved_flags: c_int,
    active: bool,
}

struct AlternateScreenGuard;

struct MouseModeGuard;

impl AlternateScreenGuard {
    fn enter() -> io::Result<Self> {
        let mut stdout = io::stdout().lock();
        stdout.write_all(ENTER_ALTERNATE_SCREEN)?;
        stdout.flush()?;
        Ok(Self)
    }
}

impl Drop for AlternateScreenGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(EXIT_ALTERNATE_SCREEN);
        let _ = stdout.flush();
    }
}

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

impl NonBlockingFdGuard {
    fn enable(fd: c_int) -> io::Result<Self> {
        let saved_flags = unsafe { fcntl(fd, F_GETFL) };
        if saved_flags == -1 {
            return Err(io::Error::last_os_error());
        }
        if saved_flags & O_NONBLOCK != 0 {
            return Ok(Self {
                fd,
                saved_flags,
                active: false,
            });
        }
        let result = unsafe { fcntl(fd, F_SETFL, saved_flags | O_NONBLOCK) };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            fd,
            saved_flags,
            active: true,
        })
    }
}

impl Drop for NonBlockingFdGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = unsafe { fcntl(self.fd, F_SETFL, self.saved_flags) };
        }
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
    fn parses_live_attach_ok_with_raw_layout_epoch() {
        assert_eq!(
            parse_attach_ok("OK\tLIVE\t42\n").unwrap(),
            AttachMode::Live {
                raw_layout_epoch: 42
            }
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
    fn reads_attach_render_frame_with_metadata_and_output_bytes() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let body = b"HEADER_ROWS\t1\nREGIONS\t1\nREGION\t2\t3\t4\t5\t6\nOUTPUT\t6\n\x1b[Habc";
        server
            .write_all(format!("FRAME\t{}\n", body.len()).as_bytes())
            .unwrap();
        server.write_all(body).unwrap();

        let frame = read_attach_render_frame(&mut client).unwrap();

        assert_eq!(frame.output, b"\x1b[Habc");
        assert_eq!(frame.header_rows, 1);
        assert_eq!(
            frame.regions,
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
    fn rejects_attach_render_frame_with_extra_output_bytes() {
        let body = b"HEADER_ROWS\t0\nREGIONS\t0\nOUTPUT\t3\nabcd";

        let err = parse_attach_render_frame_body(body).unwrap_err();

        assert!(err.to_string().contains("extra render output"));
    }

    #[test]
    fn rejects_attach_render_frame_with_truncated_output_bytes() {
        let body = b"HEADER_ROWS\t0\nREGIONS\t0\nOUTPUT\t4\nabc";

        let err = parse_attach_render_frame_body(body).unwrap_err();

        assert!(err.to_string().contains("truncated render output"));
    }

    fn render_frame(output: &[u8]) -> AttachRenderFrame {
        AttachRenderFrame {
            output: output.to_vec(),
            regions: vec![AttachPaneRegion {
                pane: 0,
                row_start: 0,
                row_end: 2,
                col_start: 0,
                col_end: 10,
            }],
            header_rows: 1,
        }
    }

    #[test]
    fn diff_live_render_output_writes_full_first_frame() {
        let mut state = LiveRenderOutputState::default();
        let frame = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Kold\x1b[2;4H";

        let output = diff_live_render_output(&render_frame(frame), &mut state);

        assert_eq!(output, b"\x1b[0m\x1b[H\x1b[2Kstatus\r\n\x1b[2Kold\x1b[2;4H");
    }

    #[test]
    fn diff_live_render_output_writes_only_changed_rows_and_cursor() {
        let mut state = LiveRenderOutputState::default();
        let first = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Kold-long\x1b[2;4H";
        let second = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Knew\x1b[2;4H";
        let _ = diff_live_render_output(&render_frame(first), &mut state);

        let output = diff_live_render_output(&render_frame(second), &mut state);

        assert!(!output.starts_with(CURSOR_HOME), "{output:?}");
        assert!(output.starts_with(b"\x1b[2;1H\x1b[0m"), "{output:?}");
        assert!(
            output
                .windows(CLEAR_LINE.len())
                .any(|window| window == CLEAR_LINE)
        );
        assert!(output.windows(b"new".len()).any(|window| window == b"new"));
        assert!(
            !output
                .windows(b"status".len())
                .any(|window| window == b"status")
        );
        assert!(output.ends_with(b"\x1b[2;4H"), "{output:?}");
    }

    #[test]
    fn diff_live_render_output_resets_style_before_clearing_changed_rows() {
        let mut state = LiveRenderOutputState::default();
        let first = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2K\x1b[48;5;24mstyled\x1b[0m\x1b[2;4H";
        let second = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Kplain\x1b[2;4H";
        let _ = diff_live_render_output(&render_frame(first), &mut state);

        let output = diff_live_render_output(&render_frame(second), &mut state);

        assert!(
            output.starts_with(b"\x1b[2;1H\x1b[0m\x1b[2Kplain"),
            "{output:?}"
        );
    }

    #[test]
    fn diff_live_render_output_writes_full_frame_after_reset() {
        let mut state = LiveRenderOutputState::default();
        let first = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Kold";
        let second = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Knew";
        let _ = diff_live_render_output(&render_frame(first), &mut state);

        reset_live_render_output_state(&mut state);
        let output = diff_live_render_output(&render_frame(second), &mut state);

        assert_eq!(output, b"\x1b[0m\x1b[H\x1b[2Kstatus\r\n\x1b[2Knew");
    }

    #[test]
    fn diff_live_render_output_writes_nothing_for_identical_frame() {
        let mut state = LiveRenderOutputState::default();
        let frame = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Ksame\x1b[2;5H";
        let _ = diff_live_render_output(&render_frame(frame), &mut state);

        let output = diff_live_render_output(&render_frame(frame), &mut state);

        assert!(output.is_empty(), "{output:?}");
    }

    #[test]
    fn diff_live_render_output_writes_only_cursor_when_rows_match() {
        let mut state = LiveRenderOutputState::default();
        let first = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Ksame\x1b[2;5H";
        let second = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Ksame\x1b[2;6H";
        let _ = diff_live_render_output(&render_frame(first), &mut state);

        let output = diff_live_render_output(&render_frame(second), &mut state);

        assert_eq!(output, b"\x1b[2;6H");
    }

    #[test]
    fn diff_live_render_output_writes_cursor_visibility_when_rows_match() {
        let mut state = LiveRenderOutputState::default();
        let first = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Ksame\x1b[?25h\x1b[2;5H";
        let second = b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Ksame\x1b[?25l\x1b[2;5H";
        let _ = diff_live_render_output(&render_frame(first), &mut state);

        let output = diff_live_render_output(&render_frame(second), &mut state);

        assert_eq!(output, b"\x1b[?25l\x1b[2;5H");
    }

    #[test]
    fn diff_live_render_output_writes_full_frame_after_geometry_change() {
        let mut state = LiveRenderOutputState::default();
        let first = render_frame(b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Kold");
        let second = AttachRenderFrame {
            output: b"\x1b[H\x1b[2Kstatus\r\n\x1b[2Knew".to_vec(),
            regions: Vec::new(),
            header_rows: 1,
        };
        let _ = diff_live_render_output(&first, &mut state);

        let output = diff_live_render_output(&second, &mut state);

        assert_eq!(output, b"\x1b[0m\x1b[H\x1b[2Kstatus\r\n\x1b[2Knew");
    }

    #[test]
    fn read_attach_layout_snapshot_falls_back_to_plain_snapshot_for_unknown_request() {
        let dir = std::env::temp_dir();
        let socket = dir.join(format!(
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

    #[test]
    fn live_snapshot_timeout_uses_polling_when_events_are_inactive() {
        assert_eq!(
            live_snapshot_redraw_timeout(false, false, None, None, Instant::now()),
            LIVE_SNAPSHOT_REDRAW_INTERVAL
        );
    }

    #[test]
    fn live_snapshot_timeout_uses_safety_interval_when_events_are_active() {
        assert_eq!(
            live_snapshot_redraw_timeout(true, false, None, None, Instant::now()),
            LIVE_SNAPSHOT_EVENT_SAFETY_REDRAW_INTERVAL
        );
    }

    #[test]
    fn live_snapshot_timeout_shortens_for_message_expiry() {
        let now = Instant::now();

        assert_eq!(
            live_snapshot_redraw_timeout(
                true,
                false,
                Some(now + Duration::from_millis(25)),
                None,
                now
            ),
            Duration::from_millis(25)
        );
    }

    #[test]
    fn live_snapshot_timeout_redraws_immediately_for_expired_message() {
        let now = Instant::now();

        assert_eq!(
            live_snapshot_redraw_timeout(true, false, Some(now), None, now),
            Duration::ZERO
        );
    }

    #[test]
    fn live_snapshot_timeout_ignores_message_expiry_while_paused() {
        let now = Instant::now();

        assert_eq!(
            live_snapshot_redraw_timeout(true, true, Some(now), Some(now), now),
            LIVE_SNAPSHOT_EVENT_SAFETY_REDRAW_INTERVAL
        );
    }

    #[test]
    fn live_snapshot_timeout_shortens_for_pending_input_repaint_deadline() {
        let now = Instant::now();

        assert_eq!(
            live_snapshot_redraw_timeout(
                true,
                false,
                None,
                Some(now + Duration::from_millis(25)),
                now
            ),
            Duration::from_millis(25)
        );
    }

    #[test]
    fn live_snapshot_timeout_redraws_immediately_for_expired_input_repaint_deadline() {
        let now = Instant::now();

        assert_eq!(
            live_snapshot_redraw_timeout(true, false, None, Some(now), now),
            Duration::ZERO
        );
    }

    #[test]
    fn render_stream_ready_does_not_activate_safety_timeout_before_first_frame() {
        let mut active = false;

        handle_render_stream_ready(&mut active);
        assert!(!active);

        handle_render_frame_received(&mut active);
        assert!(active);
    }

    #[test]
    fn render_stream_ready_does_not_clear_existing_active_fallback_stream() {
        let mut active = true;

        handle_render_stream_ready(&mut active);

        assert!(active);
    }

    #[test]
    fn live_snapshot_redraw_hint_coalesces_until_pending_clears() {
        let (sender, receiver) = mpsc::channel();
        let pending = AtomicBool::new(false);

        assert_eq!(
            send_live_snapshot_redraw_hint(&sender, &pending),
            LiveSnapshotRedrawHintStatus::Sent
        );
        assert!(matches!(
            receiver.try_recv().unwrap(),
            LiveSnapshotInputEvent::RedrawHint
        ));
        assert_eq!(
            send_live_snapshot_redraw_hint(&sender, &pending),
            LiveSnapshotRedrawHintStatus::Coalesced
        );
        assert!(receiver.try_recv().is_err());

        pending.store(false, Ordering::SeqCst);

        assert_eq!(
            send_live_snapshot_redraw_hint(&sender, &pending),
            LiveSnapshotRedrawHintStatus::Sent
        );
        assert!(matches!(
            receiver.try_recv().unwrap(),
            LiveSnapshotInputEvent::RedrawHint
        ));
    }

    #[test]
    fn live_snapshot_redraw_hint_reports_disconnected_receiver() {
        let (sender, receiver) = mpsc::channel();
        let pending = AtomicBool::new(false);
        drop(receiver);

        assert_eq!(
            send_live_snapshot_redraw_hint(&sender, &pending),
            LiveSnapshotRedrawHintStatus::Disconnected
        );
        assert!(!pending.load(Ordering::SeqCst));
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
    fn live_snapshot_input_forwards_non_press_mouse_events() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x1b[<0;1;2m", &mut state),
            vec![LiveSnapshotInputAction::Forward(b"\x1b[<0;1;2m".to_vec())]
        );
        assert_eq!(
            translate_live_snapshot_input(b"\x1b[<32;1;2M", &mut state),
            vec![LiveSnapshotInputAction::Forward(b"\x1b[<32;1;2M".to_vec())]
        );
        assert_eq!(
            translate_live_snapshot_input(b"\x1b[<64;1;2M", &mut state),
            vec![LiveSnapshotInputAction::Forward(b"\x1b[<64;1;2M".to_vec())]
        );
    }

    #[test]
    fn live_snapshot_input_forwards_modified_left_clicks() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x1b[<4;1;2M", &mut state),
            vec![LiveSnapshotInputAction::Forward(b"\x1b[<4;1;2M".to_vec())]
        );
        assert_eq!(
            translate_live_snapshot_input(b"\x1b[<8;1;2M", &mut state),
            vec![LiveSnapshotInputAction::Forward(b"\x1b[<8;1;2M".to_vec())]
        );
        assert_eq!(
            translate_live_snapshot_input(b"\x1b[<16;1;2M", &mut state),
            vec![LiveSnapshotInputAction::Forward(b"\x1b[<16;1;2M".to_vec())]
        );
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
    fn live_snapshot_input_detaches_on_prefix_uppercase_d() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02D", &mut state);

        assert_eq!(actions, vec![LiveSnapshotInputAction::Detach]);
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_forwards_literal_prefix_with_regular_key() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02a", &mut state);

        assert_eq!(
            actions,
            vec![LiveSnapshotInputAction::Forward(b"\x02a".to_vec())]
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
    fn live_snapshot_input_uses_custom_prefix_and_rebound_keys() {
        let mut state = LiveSnapshotInputState::default();
        let controls = LiveControls::from_entries(
            "C-a",
            vec![protocol::KeyBinding {
                key: "m".to_string(),
                command: "copy-mode".to_string(),
            }],
        );

        assert_eq!(
            translate_live_snapshot_input_with_mouse_and_controls(
                b"\x02m", &mut state, true, &controls
            ),
            vec![LiveSnapshotInputAction::Forward(b"\x02m".to_vec())]
        );
        assert_eq!(
            translate_live_snapshot_input_with_mouse_and_controls(
                b"\x01mrest",
                &mut state,
                true,
                &controls
            ),
            vec![LiveSnapshotInputAction::EnterCopyMode {
                initial_input: b"rest".to_vec(),
            }]
        );
    }

    #[test]
    fn live_snapshot_input_uses_bound_send_prefix_action() {
        let mut state = LiveSnapshotInputState::default();
        let controls = LiveControls::from_entries(
            "C-a",
            vec![protocol::KeyBinding {
                key: "x".to_string(),
                command: "send-prefix".to_string(),
            }],
        );

        assert_eq!(
            translate_live_snapshot_input_with_mouse_and_controls(
                b"\x01x", &mut state, true, &controls
            ),
            vec![LiveSnapshotInputAction::Forward(vec![0x01])]
        );
    }

    #[test]
    fn live_snapshot_input_unbound_key_no_longer_triggers_action() {
        let mut state = LiveSnapshotInputState::default();
        let controls = LiveControls::from_entries("C-b", Vec::new());

        assert_eq!(
            translate_live_snapshot_input_with_mouse_and_controls(
                b"\x02d", &mut state, true, &controls
            ),
            vec![LiveSnapshotInputAction::Forward(b"\x02d".to_vec())]
        );
        assert_eq!(
            translate_live_snapshot_input_with_mouse_and_controls(
                b"\x02\x02",
                &mut state,
                true,
                &controls
            ),
            vec![LiveSnapshotInputAction::Forward(b"\x02\x02".to_vec())]
        );
    }

    #[test]
    fn live_snapshot_input_selects_next_pane_on_prefix_o() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02o", &mut state);

        assert_eq!(actions, vec![LiveSnapshotInputAction::SelectNextPane]);
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_input_uses_prefix_arrow_keys_for_pane_focus() {
        let cases = [
            (b"\x02\x1b[D".as_slice(), PaneCommand::FocusLeft),
            (b"\x02\x1b[B".as_slice(), PaneCommand::FocusDown),
            (b"\x02\x1b[A".as_slice(), PaneCommand::FocusUp),
            (b"\x02\x1b[C".as_slice(), PaneCommand::FocusRight),
        ];

        for (input, command) in cases {
            let mut state = LiveSnapshotInputState::default();
            assert_eq!(
                translate_live_snapshot_input(input, &mut state),
                vec![LiveSnapshotInputAction::PaneCommand(command)]
            );
            assert!(!state.saw_prefix);
        }
    }

    #[test]
    fn live_snapshot_input_uses_alt_keys_for_pane_focus_without_prefix() {
        let cases = [
            (b"\x1bh".as_slice(), PaneCommand::FocusLeft),
            (b"\x1bj".as_slice(), PaneCommand::FocusDown),
            (b"\x1bk".as_slice(), PaneCommand::FocusUp),
            (b"\x1bl".as_slice(), PaneCommand::FocusRight),
            (b"\x1b[1;3D".as_slice(), PaneCommand::FocusLeft),
            (b"\x1b[1;3B".as_slice(), PaneCommand::FocusDown),
            (b"\x1b[1;3A".as_slice(), PaneCommand::FocusUp),
            (b"\x1b[1;3C".as_slice(), PaneCommand::FocusRight),
        ];

        for (input, command) in cases {
            let mut state = LiveSnapshotInputState::default();
            assert_eq!(
                translate_live_snapshot_input(input, &mut state),
                vec![LiveSnapshotInputAction::PaneCommand(command)]
            );
            assert!(!state.saw_prefix);
        }
    }

    #[test]
    fn live_snapshot_input_uses_prefix_ctrl_arrow_keys_for_fine_resize() {
        let cases = [
            (
                b"\x02\x1b[1;5D".as_slice(),
                protocol::PaneResizeDirection::Left,
            ),
            (
                b"\x02\x1b[1;5B".as_slice(),
                protocol::PaneResizeDirection::Down,
            ),
            (
                b"\x02\x1b[1;5A".as_slice(),
                protocol::PaneResizeDirection::Up,
            ),
            (
                b"\x02\x1b[1;5C".as_slice(),
                protocol::PaneResizeDirection::Right,
            ),
        ];

        for (input, direction) in cases {
            let mut state = LiveSnapshotInputState::default();
            assert_eq!(
                translate_live_snapshot_input(input, &mut state),
                vec![LiveSnapshotInputAction::PaneCommand(PaneCommand::Resize {
                    direction,
                    amount: 1,
                })]
            );
            assert!(!state.saw_prefix);
        }
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
    fn live_snapshot_input_dispatches_prefix_colon_command_prompt() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"\x02:rename-window work\r", &mut state);

        assert_eq!(
            actions.first(),
            Some(&LiveSnapshotInputAction::CommandPromptStart)
        );
        assert!(
            actions.contains(&LiveSnapshotInputAction::CommandPromptUpdate(
                "rename-window work".to_string()
            ))
        );
        assert_eq!(
            actions.last(),
            Some(&LiveSnapshotInputAction::CommandPromptDispatch {
                command: "rename-window work".to_string(),
                trailing_input: Vec::new(),
            })
        );
        assert!(state.command_prompt.is_none());
    }

    #[test]
    fn live_snapshot_command_prompt_dispatch_preserves_trailing_input_for_reparse() {
        let mut state = LiveSnapshotInputState::default();
        let controls = LiveControls::from_entries(
            "C-b",
            vec![
                protocol::KeyBinding {
                    key: ":".to_string(),
                    command: "command-prompt".to_string(),
                },
                protocol::KeyBinding {
                    key: "%".to_string(),
                    command: "split-window -h".to_string(),
                },
            ],
        );

        let actions = translate_live_snapshot_input_with_mouse_and_controls(
            b"\x02:set-option prefix C-a\n\x01%",
            &mut state,
            true,
            &controls,
        );

        assert_eq!(
            actions.last(),
            Some(&LiveSnapshotInputAction::CommandPromptDispatch {
                command: "set-option prefix C-a".to_string(),
                trailing_input: b"\x01%".to_vec(),
            })
        );

        let updated_controls = LiveControls::from_entries(
            "C-a",
            vec![protocol::KeyBinding {
                key: "%".to_string(),
                command: "split-window -h".to_string(),
            }],
        );
        assert_eq!(
            translate_live_snapshot_input_with_mouse_and_controls(
                b"\x01%",
                &mut state,
                true,
                &updated_controls,
            ),
            vec![LiveSnapshotInputAction::PaneCommand(
                PaneCommand::SplitRight
            )]
        );
    }

    #[test]
    fn live_snapshot_input_forwards_drag_release_and_wheel_mouse_events() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input_with_mouse(
                b"\x1b[<32;2;3M\x1b[<0;2;3m\x1b[<64;2;3M",
                &mut state,
                true,
            ),
            vec![LiveSnapshotInputAction::Forward(
                b"\x1b[<32;2;3M\x1b[<0;2;3m\x1b[<64;2;3M".to_vec()
            )]
        );
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
    fn attach_input_translates_pane_commands() {
        let cases = [
            (b'c', PaneCommand::NewWindow),
            (b'n', PaneCommand::NextWindow),
            (b'p', PaneCommand::PreviousWindow),
            (b'%', PaneCommand::SplitRight),
            (b'"', PaneCommand::SplitDown),
            (b'h', PaneCommand::FocusLeft),
            (b'j', PaneCommand::FocusDown),
            (b'k', PaneCommand::FocusUp),
            (b'l', PaneCommand::FocusRight),
            (b'x', PaneCommand::Close),
            (b'z', PaneCommand::ToggleZoom),
        ];

        for (key, command) in cases {
            assert_eq!(
                translate_attach_input(&[0x02, key], &mut false),
                vec![AttachInputAction::PaneCommand(command)]
            );
        }
    }

    #[test]
    fn attach_input_uses_prefix_arrow_keys_for_pane_focus() {
        let cases = [
            (b"\x02\x1b[D".as_slice(), PaneCommand::FocusLeft),
            (b"\x02\x1b[B".as_slice(), PaneCommand::FocusDown),
            (b"\x02\x1b[A".as_slice(), PaneCommand::FocusUp),
            (b"\x02\x1b[C".as_slice(), PaneCommand::FocusRight),
        ];

        for (input, command) in cases {
            assert_eq!(
                translate_attach_input(input, &mut false),
                vec![AttachInputAction::PaneCommand(command)]
            );
        }
    }

    #[test]
    fn attach_input_uses_alt_keys_for_pane_focus_without_prefix() {
        let cases = [
            (b"\x1bh".as_slice(), PaneCommand::FocusLeft),
            (b"\x1bj".as_slice(), PaneCommand::FocusDown),
            (b"\x1bk".as_slice(), PaneCommand::FocusUp),
            (b"\x1bl".as_slice(), PaneCommand::FocusRight),
            (b"\x1b[1;3D".as_slice(), PaneCommand::FocusLeft),
            (b"\x1b[1;3B".as_slice(), PaneCommand::FocusDown),
            (b"\x1b[1;3A".as_slice(), PaneCommand::FocusUp),
            (b"\x1b[1;3C".as_slice(), PaneCommand::FocusRight),
        ];

        for (input, command) in cases {
            assert_eq!(
                translate_attach_input(input, &mut false),
                vec![AttachInputAction::PaneCommand(command)]
            );
        }
    }

    #[test]
    fn attach_input_dispatches_prefix_colon_command_prompt() {
        let mut state = RawAttachInputState::default();
        let actions = translate_attach_input_with_state(b"\x02:select-window 1\n", &mut state);

        assert_eq!(
            actions.first(),
            Some(&AttachInputAction::CommandPromptStart)
        );
        assert!(actions.contains(&AttachInputAction::CommandPromptUpdate(
            "select-window 1".to_string()
        )));
        assert_eq!(
            actions.last(),
            Some(&AttachInputAction::CommandPromptDispatch {
                command: "select-window 1".to_string(),
                trailing_input: Vec::new(),
            })
        );
        assert!(state.command_prompt.is_none());
    }

    #[test]
    fn attach_command_prompt_dispatch_preserves_trailing_input_for_reparse() {
        let mut state = RawAttachInputState::default();
        let controls = LiveControls::from_entries(
            "C-b",
            vec![
                protocol::KeyBinding {
                    key: ":".to_string(),
                    command: "command-prompt".to_string(),
                },
                protocol::KeyBinding {
                    key: "%".to_string(),
                    command: "split-window -h".to_string(),
                },
            ],
        );

        let actions = translate_attach_input_with_state_with_controls(
            b"\x02:set-option prefix C-a\n\x01%",
            &mut state,
            &controls,
        );

        assert_eq!(
            actions.last(),
            Some(&AttachInputAction::CommandPromptDispatch {
                command: "set-option prefix C-a".to_string(),
                trailing_input: b"\x01%".to_vec(),
            })
        );
    }

    #[test]
    fn attach_command_prompt_drops_mouse_sequence_on_cancel() {
        let mut state = RawAttachInputState::default();

        assert_eq!(
            translate_attach_input_with_state(b"\x02:", &mut state),
            vec![AttachInputAction::CommandPromptStart]
        );
        assert_eq!(
            translate_attach_input_with_state(b"\x1b[<0;12;3M", &mut state),
            vec![AttachInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn attach_command_prompt_buffers_split_mouse_sequence_before_cancel() {
        let mut state = RawAttachInputState::default();

        assert_eq!(
            translate_attach_input_with_state(b"\x02:", &mut state),
            vec![AttachInputAction::CommandPromptStart]
        );
        assert!(translate_attach_input_with_state(b"\x1b[<0;12", &mut state).is_empty());
        assert!(state.command_prompt.is_some());
        assert!(!state.mouse_pending.is_empty());
        assert_eq!(
            translate_attach_input_with_state(b";3M", &mut state),
            vec![AttachInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn attach_command_prompt_buffers_mouse_sequence_split_after_escape() {
        let mut state = RawAttachInputState::default();

        assert_eq!(
            translate_attach_input_with_state(b"\x02:", &mut state),
            vec![AttachInputAction::CommandPromptStart]
        );
        assert!(translate_attach_input_with_state(b"\x1b", &mut state).is_empty());
        assert!(state.command_prompt.is_some());
        assert_eq!(
            translate_attach_input_with_state(b"[<0;12;3M", &mut state),
            vec![AttachInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn attach_command_prompt_drops_csi_sequence_on_cancel() {
        let mut state = RawAttachInputState::default();

        assert_eq!(
            translate_attach_input_with_state(b"\x02:", &mut state),
            vec![AttachInputAction::CommandPromptStart]
        );
        assert_eq!(
            translate_attach_input_with_state(b"\x1b[D", &mut state),
            vec![AttachInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn attach_command_prompt_buffers_split_csi_sequence_before_cancel() {
        let mut state = RawAttachInputState::default();

        assert_eq!(
            translate_attach_input_with_state(b"\x02:", &mut state),
            vec![AttachInputAction::CommandPromptStart]
        );
        assert!(translate_attach_input_with_state(b"\x1b[1", &mut state).is_empty());
        assert!(state.command_prompt.is_some());
        assert!(!state.mouse_pending.is_empty());
        assert_eq!(
            translate_attach_input_with_state(b";5D", &mut state),
            vec![AttachInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn attach_command_prompt_buffers_csi_sequence_split_after_escape() {
        let mut state = RawAttachInputState::default();

        assert_eq!(
            translate_attach_input_with_state(b"\x02:", &mut state),
            vec![AttachInputAction::CommandPromptStart]
        );
        assert!(translate_attach_input_with_state(b"\x1b", &mut state).is_empty());
        assert!(state.command_prompt.is_some());
        assert_eq!(
            translate_attach_input_with_state(b"[D", &mut state),
            vec![AttachInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn attach_command_prompt_cancels_pending_escape_after_timeout() {
        let mut state = RawAttachInputState::default();

        assert_eq!(
            translate_attach_input_with_state(b"\x02:", &mut state),
            vec![AttachInputAction::CommandPromptStart]
        );
        assert!(translate_attach_input_with_state(b"\x1b", &mut state).is_empty());
        let pending_since = state
            .prompt_pending_since
            .expect("pending escape timestamp");
        assert_eq!(
            state.prompt_pending_timeout_action(pending_since + COPY_MODE_ESCAPE_DISAMBIGUATION),
            Some(AttachInputAction::CommandPromptCancel)
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn directional_focus_uses_server_select_request() {
        let socket = std::path::PathBuf::from(format!(
            "/tmp/dmux-focus-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn({
            let socket = socket.clone();
            move || {
                let (mut stream, _) = listener.accept().unwrap();
                assert_eq!(
                    read_line(&mut stream).unwrap(),
                    "SELECT_PANE_DIRECTION\tdev\tL\n"
                );
                stream.write_all(b"OK\n").unwrap();
                let _ = std::fs::remove_file(socket);
            }
        });

        select_directional_pane(&socket, "dev", PaneCommand::FocusLeft).unwrap();

        server.join().unwrap();
    }

    #[test]
    fn attach_prompt_rename_session_does_not_force_raw_reconnect() {
        let socket = std::path::PathBuf::from(format!(
            "/tmp/dmux-rename-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn({
            let socket = socket.clone();
            move || {
                let (mut stream, _) = listener.accept().unwrap();
                assert_eq!(
                    read_line(&mut stream).unwrap(),
                    protocol::encode_rename_session("dev", "work")
                );
                stream.write_all(b"OK\n").unwrap();
                let _ = std::fs::remove_file(socket);
            }
        });

        let result = dispatch_attach_command(&socket, "dev", "rename-session work").unwrap();

        assert!(result.message.is_none());
        assert!(!result.reconnect);
        server.join().unwrap();
    }

    #[test]
    fn attach_prompt_kill_session_marks_attach_exit() {
        let socket = std::path::PathBuf::from(format!(
            "/tmp/dmux-kill-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn({
            let socket = socket.clone();
            move || {
                let (mut stream, _) = listener.accept().unwrap();
                assert_eq!(
                    read_line(&mut stream).unwrap(),
                    protocol::encode_kill("dev")
                );
                stream.write_all(b"OK\n").unwrap();
                let _ = std::fs::remove_file(socket);
            }
        });

        let result = dispatch_attach_command(&socket, "dev", "kill-session").unwrap();

        assert!(result.message.is_none());
        assert!(!result.reconnect);
        assert!(result.exit_attach);
        server.join().unwrap();
    }

    #[test]
    fn attach_prompt_list_keys_and_show_options_parse_format_args() {
        let socket = std::path::PathBuf::from(format!(
            "/tmp/dmux-list-keys-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn({
            let socket = socket.clone();
            move || {
                {
                    let (mut stream, _) = listener.accept().unwrap();
                    assert_eq!(
                        read_line(&mut stream).unwrap(),
                        protocol::encode_list_keys(Some("#{key}=#{command}"))
                    );
                    stream.write_all(b"OK\nC-b=send-prefix\n").unwrap();
                }

                {
                    let (mut stream, _) = listener.accept().unwrap();
                    assert_eq!(
                        read_line(&mut stream).unwrap(),
                        protocol::encode_show_options(Some("#{option.name}=#{option.value}"))
                    );
                    stream.write_all(b"OK\nprefix=C-b\n").unwrap();
                }
                let _ = std::fs::remove_file(socket);
            }
        });

        let keys =
            dispatch_attach_command(&socket, "dev", "list-keys -F '#{key}=#{command}'").unwrap();
        assert_eq!(keys.message.as_deref(), Some("C-b=send-prefix"));
        let options = dispatch_attach_command(
            &socket,
            "dev",
            "show-options -F '#{option.name}=#{option.value}'",
        )
        .unwrap();
        assert_eq!(options.message.as_deref(), Some("prefix=C-b"));
        server.join().unwrap();

        let err = match dispatch_attach_command(Path::new("unused.sock"), "dev", "list-keys bad") {
            Ok(_) => panic!("list-keys with invalid args should fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("does not support argument"),
            "{}",
            err
        );
    }

    #[test]
    fn attach_prompt_layout_requires_preset_before_contacting_server() {
        let err = match dispatch_attach_command(Path::new("unused.sock"), "dev", "layout") {
            Ok(_) => panic!("layout without preset should fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("requires a preset"), "{}", err);
    }

    #[test]
    fn attach_source_file_rejects_recursive_source_file() {
        let path = std::env::temp_dir().join(format!(
            "dmux-recursive-source-{}-{}.dmux",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, format!("source-file {}\n", path.display())).unwrap();

        let err = match dispatch_attach_command(
            Path::new("unused.sock"),
            "dev",
            &format!("source-file {}", path.display()),
        ) {
            Ok(_) => panic!("recursive source-file should fail"),
            Err(err) => err,
        };

        let message = err.to_string();
        assert!(
            message.contains("nested source-file is not allowed in attach scripts"),
            "{message}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn attach_input_preserves_forwarded_bytes_before_pane_command() {
        let actions = translate_attach_input(b"abc\x02%def", &mut false);

        assert_eq!(
            actions,
            vec![
                AttachInputAction::Forward(b"abc".to_vec()),
                AttachInputAction::PaneCommand(PaneCommand::SplitRight),
                AttachInputAction::Forward(b"def".to_vec()),
            ]
        );
    }

    #[test]
    fn attach_input_sends_literal_prefix_without_retaining_prefix() {
        let actions = translate_attach_input(b"\x02\x02d", &mut false);

        assert_eq!(actions, vec![AttachInputAction::Forward(b"\x02d".to_vec())]);
    }

    #[test]
    fn attach_input_uses_bound_send_prefix_action() {
        let mut state = RawAttachInputState::default();
        let controls = LiveControls::from_entries(
            "C-a",
            vec![protocol::KeyBinding {
                key: "x".to_string(),
                command: "send-prefix".to_string(),
            }],
        );

        let actions =
            translate_attach_input_with_state_with_controls(b"\x01x", &mut state, &controls);

        assert_eq!(actions, vec![AttachInputAction::Forward(vec![0x01])]);
    }

    #[test]
    fn attach_input_unbound_prefix_key_forwards_prefix_and_key() {
        let mut state = RawAttachInputState::default();
        let controls = LiveControls::from_entries("C-b", Vec::new());

        let actions =
            translate_attach_input_with_state_with_controls(b"\x02\x02", &mut state, &controls);

        assert_eq!(
            actions,
            vec![AttachInputAction::Forward(b"\x02\x02".to_vec())]
        );
        assert!(!state.saw_prefix);
    }

    #[test]
    fn attach_input_consumes_select_next_prefix_without_forwarding() {
        let actions = translate_attach_input(b"\x02o", &mut false);

        assert_eq!(actions, vec![AttachInputAction::SelectNextPane]);
    }

    #[test]
    fn attach_input_consumes_pane_number_selection_without_forwarding() {
        let actions = translate_attach_input(b"\x02q0", &mut false);

        assert_eq!(
            actions,
            vec![
                AttachInputAction::ShowPaneNumbers,
                AttachInputAction::SelectPane(0)
            ]
        );
    }

    #[test]
    fn attach_input_selects_numbered_pane_after_prefix_q_across_reads() {
        let mut state = RawAttachInputState::default();

        assert_eq!(
            translate_attach_input_with_state(b"\x02q", &mut state),
            vec![AttachInputAction::ShowPaneNumbers]
        );
        assert_eq!(
            translate_attach_input_with_state(b"0", &mut state),
            vec![AttachInputAction::SelectPane(0)]
        );
    }

    #[test]
    fn raw_pending_input_preserves_focus_commands_after_layout_transition() {
        let pending = raw_pending_input(
            &[
                AttachInputAction::PaneCommand(PaneCommand::FocusLeft),
                AttachInputAction::PaneCommand(PaneCommand::FocusRight),
                AttachInputAction::Forward(b"tail".to_vec()),
            ],
            false,
            RawPendingFocus::Preserve,
            &LiveControls::default(),
        );

        assert_eq!(pending, b"\x02h\x02ltail");
    }

    #[test]
    fn raw_pending_input_preserves_focus_commands_after_zoom_transition() {
        let pending = raw_pending_input(
            &[
                AttachInputAction::PaneCommand(PaneCommand::FocusLeft),
                AttachInputAction::Forward(b"tail".to_vec()),
            ],
            false,
            RawPendingFocus::Preserve,
            &LiveControls::default(),
        );

        assert_eq!(pending, b"\x02htail");
    }

    #[test]
    fn raw_pending_input_uses_custom_prefix_and_rebound_keys() {
        let controls = LiveControls::from_entries(
            "C-a",
            vec![
                protocol::KeyBinding {
                    key: "m".to_string(),
                    command: "new-window".to_string(),
                },
                protocol::KeyBinding {
                    key: "s".to_string(),
                    command: "split-window -h".to_string(),
                },
            ],
        );
        let actions = translate_attach_input_with_state_with_controls(
            b"\x01m\x01s",
            &mut RawAttachInputState::default(),
            &controls,
        );
        assert_eq!(
            actions,
            vec![
                AttachInputAction::PaneCommand(PaneCommand::NewWindow),
                AttachInputAction::PaneCommand(PaneCommand::SplitRight),
            ]
        );

        let pending = raw_pending_input(&actions[1..], false, RawPendingFocus::Preserve, &controls);

        assert_eq!(pending, b"\x01s");
    }

    #[test]
    fn raw_pending_input_preserves_trailing_prefix_after_layout_transition() {
        let pending = raw_pending_input(&[], true, RawPendingFocus::Drop, &LiveControls::default());

        assert_eq!(pending, b"\x02");
    }

    #[test]
    fn live_snapshot_input_translates_pane_commands() {
        let cases = [
            (b'%', PaneCommand::SplitRight),
            (b'"', PaneCommand::SplitDown),
            (b'h', PaneCommand::FocusLeft),
            (b'j', PaneCommand::FocusDown),
            (b'k', PaneCommand::FocusUp),
            (b'l', PaneCommand::FocusRight),
            (b'x', PaneCommand::Close),
            (b'z', PaneCommand::ToggleZoom),
        ];

        for (key, command) in cases {
            let mut state = LiveSnapshotInputState::default();
            let actions = translate_live_snapshot_input(&[0x02, key], &mut state);

            assert_eq!(actions, vec![LiveSnapshotInputAction::PaneCommand(command)]);
            assert!(!state.saw_prefix);
        }
    }

    #[test]
    fn live_snapshot_input_preserves_forwarded_bytes_before_pane_command() {
        let mut state = LiveSnapshotInputState::default();

        let actions = translate_live_snapshot_input(b"abc\x02ldef", &mut state);

        assert_eq!(
            actions,
            vec![
                LiveSnapshotInputAction::Forward(b"abc".to_vec()),
                LiveSnapshotInputAction::PaneCommand(PaneCommand::FocusRight),
                LiveSnapshotInputAction::Forward(b"def".to_vec()),
            ]
        );
        assert!(!state.saw_prefix);
    }

    #[test]
    fn live_snapshot_command_prompt_drops_mouse_sequence_on_cancel() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x02:", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptStart]
        );
        assert_eq!(
            translate_live_snapshot_input(b"\x1b[<0;12;3M", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn live_snapshot_command_prompt_buffers_split_mouse_sequence_before_cancel() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x02:", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptStart]
        );
        assert!(translate_live_snapshot_input(b"\x1b[<0;12", &mut state).is_empty());
        assert!(state.command_prompt.is_some());
        assert!(!state.mouse_pending.is_empty());
        assert_eq!(
            translate_live_snapshot_input(b";3M", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn live_snapshot_command_prompt_buffers_mouse_sequence_split_after_escape() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x02:", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptStart]
        );
        assert!(translate_live_snapshot_input(b"\x1b", &mut state).is_empty());
        assert!(state.command_prompt.is_some());
        assert_eq!(
            translate_live_snapshot_input(b"[<0;12;3M", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn live_snapshot_command_prompt_drops_csi_sequence_on_cancel() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x02:", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptStart]
        );
        assert_eq!(
            translate_live_snapshot_input(b"\x1b[D", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn live_snapshot_command_prompt_buffers_split_csi_sequence_before_cancel() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x02:", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptStart]
        );
        assert!(translate_live_snapshot_input(b"\x1b[1", &mut state).is_empty());
        assert!(state.command_prompt.is_some());
        assert!(!state.mouse_pending.is_empty());
        assert_eq!(
            translate_live_snapshot_input(b";5D", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn live_snapshot_command_prompt_buffers_csi_sequence_split_after_escape() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x02:", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptStart]
        );
        assert!(translate_live_snapshot_input(b"\x1b", &mut state).is_empty());
        assert!(state.command_prompt.is_some());
        assert_eq!(
            translate_live_snapshot_input(b"[D", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptCancel]
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
    }

    #[test]
    fn live_snapshot_command_prompt_cancels_pending_escape_after_timeout() {
        let mut state = LiveSnapshotInputState::default();

        assert_eq!(
            translate_live_snapshot_input(b"\x02:", &mut state),
            vec![LiveSnapshotInputAction::CommandPromptStart]
        );
        assert!(translate_live_snapshot_input(b"\x1b", &mut state).is_empty());
        let pending_since = state
            .prompt_pending_since
            .expect("pending escape timestamp");
        assert_eq!(
            state.prompt_pending_timeout_action(pending_since + COPY_MODE_ESCAPE_DISAMBIGUATION),
            Some(LiveSnapshotInputAction::CommandPromptCancel)
        );
        assert!(state.command_prompt.is_none());
        assert!(state.mouse_pending.is_empty());
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
            command_prompt: None,
            mouse_pending: Vec::new(),
            prompt_pending_since: None,
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
    fn copy_mode_view_pages_and_clips_to_terminal_height() {
        let mut view = CopyModeView::from_numbered_output(
            "1\tone\n2\ttwo\n3\tthree\n4\tfour\n5\tfive\n6\tsix\n",
        )
        .unwrap();
        view.set_height_for_test(4);

        assert!(!view.render().contains("4\tfour"));
        assert_eq!(view.apply_key(b'\x06'), CopyModeAction::Redraw);
        assert_eq!(view.cursor_line_number(), Some(4));
        assert!(view.render().contains("4\tfour"));
        assert!(!view.render().contains("1\tone"));
    }

    #[test]
    fn copy_mode_view_home_end_jump_viewport() {
        let mut view =
            CopyModeView::from_numbered_output("1\tone\n2\ttwo\n3\tthree\n4\tfour\n").unwrap();
        view.set_height_for_test(3);

        assert_eq!(view.apply_key(b'G'), CopyModeAction::Redraw);
        assert_eq!(view.cursor_line_number(), Some(4));
        assert!(view.render().contains("4\tfour"));
        assert!(!view.render().contains("1\tone"));
        assert_eq!(view.apply_key(b'g'), CopyModeAction::Redraw);
        assert_eq!(view.cursor_line_number(), Some(1));
    }

    #[test]
    fn copy_mode_input_parses_arrow_and_page_keys() {
        let mut view =
            CopyModeView::from_numbered_output("1\tone\n2\ttwo\n3\tthree\n4\tfour\n").unwrap();
        let mut input = CopyModeInputState::default();

        assert_eq!(
            input.apply(&mut view, b"\x1b[B"),
            vec![CopyModeAction::Redraw]
        );
        assert_eq!(view.cursor_line_number(), Some(2));
        assert_eq!(
            input.apply(&mut view, b"\x1b[5~"),
            vec![CopyModeAction::Redraw]
        );
        assert_eq!(view.cursor_line_number(), Some(1));
    }

    #[test]
    fn copy_mode_input_ignores_left_and_right_arrows() {
        let mut view = CopyModeView::from_numbered_output("1\tone\n2\ttwo\n").unwrap();
        let mut input = CopyModeInputState::default();

        assert_eq!(
            input.apply(&mut view, b"\x1b[D"),
            vec![CopyModeAction::Ignore]
        );
        assert_eq!(
            input.apply(&mut view, b"\x1b[C"),
            vec![CopyModeAction::Ignore]
        );
        assert_eq!(view.cursor_line_number(), Some(1));
    }

    #[test]
    fn copy_mode_input_buffers_csi_keys_split_after_escape() {
        let mut view =
            CopyModeView::from_numbered_output("1\tone\n2\ttwo\n3\tthree\n4\tfour\n").unwrap();
        let mut input = CopyModeInputState::default();

        assert_eq!(input.apply(&mut view, b"\x1b"), Vec::new());
        assert_eq!(input.apply(&mut view, b"[B"), vec![CopyModeAction::Redraw]);
        assert_eq!(view.cursor_line_number(), Some(2));

        assert_eq!(input.apply(&mut view, b"\x1b"), Vec::new());
        assert_eq!(input.apply(&mut view, b"[5~"), vec![CopyModeAction::Redraw]);
        assert_eq!(view.cursor_line_number(), Some(1));
    }

    #[test]
    fn copy_mode_input_ignores_split_left_arrow_after_escape() {
        let mut view = CopyModeView::from_numbered_output("1\tone\n2\ttwo\n").unwrap();
        let mut input = CopyModeInputState::default();

        assert_eq!(input.apply(&mut view, b"\x1b"), Vec::new());
        assert_eq!(input.apply(&mut view, b"[D"), vec![CopyModeAction::Ignore]);
        assert_eq!(view.cursor_line_number(), Some(1));
    }

    #[test]
    fn copy_mode_input_buffers_split_unsupported_csi_parameters() {
        let mut view = CopyModeView::from_numbered_output("1\tone\n2\ttwo\n").unwrap();
        let mut input = CopyModeInputState::default();

        assert_eq!(input.apply(&mut view, b"\x1b[1"), Vec::new());
        assert_eq!(input.apply(&mut view, b";5D"), vec![CopyModeAction::Ignore]);
        assert_eq!(view.cursor_line_number(), Some(1));
    }

    #[test]
    fn copy_mode_input_exits_on_lone_escape_after_disambiguation_timeout() {
        let mut view = CopyModeView::from_numbered_output("1\tone\n").unwrap();
        let mut input = CopyModeInputState::default();

        assert_eq!(input.apply(&mut view, b"\x1b"), Vec::new());
        let pending_since = input.pending_since.expect("pending escape timestamp");
        assert_eq!(
            input.pending_timeout_action(pending_since + COPY_MODE_ESCAPE_DISAMBIGUATION),
            Some(CopyModeAction::Exit)
        );
    }

    #[test]
    fn copy_mode_view_copies_current_line() {
        let mut view = CopyModeView::from_numbered_output("7\tselected\n").unwrap();

        assert_eq!(view.apply_key(b'y'), CopyModeAction::CopyLine(7));
    }

    #[test]
    fn copy_mode_view_numbers_plain_rendered_lines() {
        let view = CopyModeView::from_plain_text("base | split\r\n-----+------\r\n").unwrap();

        assert_eq!(view.cursor_line_number(), Some(1));
        assert_eq!(
            view.selected_text_for_line_range(1, 2).unwrap(),
            "base | split\n-----+------\n"
        );
    }

    #[test]
    fn copy_mode_view_preserves_blank_plain_lines() {
        let view = CopyModeView::from_plain_text("top\r\n\r\nbottom\r\n").unwrap();

        assert_eq!(view.selected_text_for_line_range(2, 2).unwrap(), "\n");
    }

    #[test]
    fn copy_mode_save_request_uses_selected_view_text() {
        let view = CopyModeView::from_numbered_output("7\tactive\n8\tpane\n").unwrap();

        assert_eq!(
            encode_copy_mode_save_request("dev", &view, 7, 8).unwrap(),
            protocol::encode_save_buffer_text("dev", None, "active\npane\n")
        );
    }

    #[test]
    fn composed_copy_mode_save_request_uses_selected_plain_text() {
        let view = CopyModeView::from_plain_text("base | split\r\nsecond\r\n").unwrap();

        assert_eq!(
            encode_copy_mode_save_request("dev", &view, 1, 1).unwrap(),
            protocol::encode_save_buffer_text("dev", None, "base | split\n")
        );
    }

    #[test]
    fn copy_mode_text_save_maps_unknown_request_to_unsupported() {
        let err = map_copy_mode_save_text_error(io::Error::other(
            "unknown request line: \"SAVE_BUFFER_TEXT\"",
        ));

        assert_eq!(
            err.to_string(),
            "copy-mode text save requires an updated dmux server"
        );
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
    fn attach_input_detaches_on_prefix_uppercase_d() {
        let actions = translate_attach_input(b"\x02D", &mut false);

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
    fn attach_help_message_reuses_cli_overlay() {
        assert_eq!(attach_help_message(), crate::cli::attach_help_overlay());
        assert!(attach_help_message().contains("Session:"));
        assert!(attach_help_message().contains("Prompt examples:"));
        assert!(
            attach_help_message()
                .contains("copy-mode: j/k arrows PgUp/PgDn y/Enter copy q/Esc exit")
        );
    }

    #[test]
    fn attach_help_popup_message_is_boxed_and_has_close_hint() {
        let popup = attach_help_popup_message();

        assert!(
            popup.lines().next().unwrap().contains("dmux help"),
            "{popup}"
        );
        assert!(popup.contains("C-b ? close"), "{popup}");
        assert!(popup.contains("Alt-h/j/k/l"), "{popup}");
        assert!(popup.contains("Prompt examples:"), "{popup}");
    }

    #[test]
    fn command_prompt_message_shows_command_and_controls() {
        let message = attach_command_prompt_text("rename-window api");

        assert!(message.contains(":rename-window api"), "{message}");
        assert!(message.contains("Enter run"), "{message}");
        assert!(message.contains("Esc/C-c cancel"), "{message}");
        assert!(message.contains("split -h"), "{message}");
    }

    #[test]
    fn raw_command_prompt_display_truncates_to_terminal_cells() {
        let message = attach_command_prompt_display_text("rename-window 界界界界", Some(24));

        assert!(display_cell_width(&message) <= 24, "{message}");
        assert!(message.ends_with("..."), "{message}");
    }

    #[test]
    fn live_header_lines_truncate_to_terminal_width() {
        let lines = live_header_lines(
            "session-with-a-very-long-status-line",
            Some(&attach_command_prompt_text("rename-window api")),
            Some(20),
        );

        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.chars().count() <= 20));
        assert_eq!(lines[0], "session-with-a-ve...");
        assert!(lines[1].ends_with("..."), "{lines:?}");
    }

    #[test]
    fn live_header_lines_truncate_wide_chars_to_terminal_cells() {
        let lines = live_header_lines("界界界界", None, Some(5));

        assert_eq!(lines, vec!["界..."]);
        assert!(lines.iter().all(|line| display_cell_width(line) <= 5));
    }

    #[test]
    fn live_header_lines_count_multiline_help_rows() {
        let lines = live_header_lines("status", Some("first long line\nsecond"), Some(10));

        assert_eq!(lines, vec!["status", "first l...", "second"]);
    }

    #[test]
    fn cap_live_header_lines_reserves_snapshot_row_when_possible() {
        let lines = live_header_lines("status", Some("one\ntwo\nthree\nfour"), Some(12));

        let capped = cap_live_header_lines(lines, Some(3), Some(12), false);

        assert_eq!(capped, vec!["status", "one", "... 3 mor..."]);
    }

    #[test]
    fn cap_live_header_lines_prioritizes_active_prompt_on_tiny_terminals() {
        let lines = live_header_lines("status", Some(":rename-window api"), Some(20));

        let capped = cap_live_header_lines(lines, Some(1), Some(20), true);

        assert_eq!(capped, vec![":rename-window api"]);
    }

    #[test]
    fn active_live_message_keeps_command_prompt_visible_after_transient_expiry() {
        let now = Instant::now();
        let mut transient = Some(("old".to_string(), now));

        let message = active_live_message(&mut transient, Some(":rename-window api"), now);

        assert_eq!(message, Some(":rename-window api"));
        assert!(transient.is_none());
    }

    #[test]
    fn help_popup_overlay_does_not_become_a_header_message() {
        let now = Instant::now();
        let mut transient = None;

        let message = active_live_message(&mut transient, None, now);
        let header_lines = live_header_lines("tabs: [0:0*]\npane 0", message, Some(80));

        assert_eq!(message, None);
        assert_eq!(header_lines, vec!["tabs: [0:0*]", "pane 0"]);
    }

    #[test]
    fn popup_overlay_uses_absolute_position_and_restores_cursor() {
        let mut output = b"\x1b[0m\x1b[H\x1b[2Ktabs\r\n\x1b[2Kpane\x1b[?25h\x1b[2;5H".to_vec();

        append_centered_popup_overlay(
            &mut output,
            "dmux help",
            &["C-b ? close".to_string(), "Prompt examples:".to_string()],
            PtySize { cols: 40, rows: 10 },
            1,
        );

        assert!(
            output
                .windows(b"\x1b7".len())
                .any(|window| window == b"\x1b7")
        );
        assert!(
            output
                .windows(b"\x1b8".len())
                .any(|window| window == b"\x1b8")
        );
        assert!(
            output
                .windows(b"\x1b[5;".len())
                .any(|window| window == b"\x1b[5;")
        );
        assert!(
            output
                .windows("┌ dmux help ".as_bytes().len())
                .any(|window| { window == "┌ dmux help ".as_bytes() })
        );
        assert!(output.ends_with(b"\x1b8"), "{output:?}");
    }

    #[test]
    fn attach_input_reports_command_prompt_updates() {
        let mut state = RawAttachInputState::default();

        let actions = translate_attach_input_with_state(b"\x02:abc\x7fd", &mut state);

        assert_eq!(
            actions,
            vec![
                AttachInputAction::CommandPromptStart,
                AttachInputAction::CommandPromptUpdate("a".to_string()),
                AttachInputAction::CommandPromptUpdate("ab".to_string()),
                AttachInputAction::CommandPromptUpdate("abc".to_string()),
                AttachInputAction::CommandPromptUpdate("ab".to_string()),
                AttachInputAction::CommandPromptUpdate("abd".to_string()),
            ]
        );
        assert_eq!(state.command_prompt, Some(b"abd".to_vec()));
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
