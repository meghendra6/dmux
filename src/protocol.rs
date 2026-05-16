pub const ARG_SEPARATOR: char = '\u{1f}';
pub const MAX_SAVE_BUFFER_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Screen,
    History,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferSelection {
    All,
    LineRange { start: usize, end: usize },
    Search(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    New {
        session: String,
        command: Vec<String>,
    },
    Attach {
        session: String,
    },
    AttachRawState {
        session: String,
    },
    AttachSnapshot {
        session: String,
    },
    AttachLayoutSnapshot {
        session: String,
    },
    AttachLayoutFrame {
        session: String,
    },
    AttachEvents {
        session: String,
    },
    AttachRender {
        session: String,
    },
    List,
    Capture {
        session: String,
        mode: CaptureMode,
    },
    SaveBuffer {
        session: String,
        buffer: Option<String>,
        mode: CaptureMode,
        selection: BufferSelection,
    },
    SaveBufferText {
        session: String,
        buffer: Option<String>,
        text: String,
    },
    CopyMode {
        session: String,
        mode: CaptureMode,
        search: Option<String>,
    },
    ListBuffers,
    PasteBuffer {
        session: String,
        buffer: Option<String>,
    },
    DeleteBuffer {
        buffer: String,
    },
    Resize {
        session: String,
        cols: u16,
        rows: u16,
    },
    Send {
        session: String,
        bytes: Vec<u8>,
    },
    Split {
        session: String,
        direction: SplitDirection,
        command: Vec<String>,
    },
    ListPanes {
        session: String,
        format: Option<String>,
    },
    SelectPane {
        session: String,
        pane: usize,
    },
    KillPane {
        session: String,
        pane: Option<usize>,
    },
    NewWindow {
        session: String,
        command: Vec<String>,
    },
    ListWindows {
        session: String,
    },
    SelectWindow {
        session: String,
        window: usize,
    },
    KillWindow {
        session: String,
        window: Option<usize>,
    },
    ZoomPane {
        session: String,
        pane: Option<usize>,
    },
    StatusLine {
        session: String,
        format: Option<String>,
    },
    DisplayMessage {
        session: String,
        format: String,
    },
    Kill {
        session: String,
    },
    KillServer,
}

pub fn encode_new(session: &str, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!("NEW\t{session}\t{}\t{joined}\n", command.len())
}

pub fn encode_attach(session: &str) -> String {
    format!("ATTACH\t{session}\n")
}

pub fn encode_attach_raw_state(session: &str) -> String {
    format!("ATTACH_RAW_STATE\t{session}\n")
}

#[allow(dead_code)]
pub fn encode_attach_snapshot(session: &str) -> String {
    format!("ATTACH_SNAPSHOT\t{session}\n")
}

pub fn encode_attach_layout_snapshot(session: &str) -> String {
    format!("ATTACH_LAYOUT_SNAPSHOT\t{session}\n")
}

pub fn encode_attach_layout_frame(session: &str) -> String {
    format!("ATTACH_LAYOUT_FRAME\t{session}\n")
}

#[allow(dead_code)]
pub fn encode_attach_events(session: &str) -> String {
    format!("ATTACH_EVENTS\t{session}\n")
}

pub fn encode_attach_render(session: &str) -> String {
    format!("ATTACH_RENDER\t{session}\n")
}

pub fn encode_list() -> &'static str {
    "LIST\n"
}

pub fn encode_capture(session: &str, mode: CaptureMode) -> String {
    format!("CAPTURE\t{session}\t{}\n", encode_capture_mode(mode))
}

pub fn encode_save_buffer(
    session: &str,
    buffer: Option<&str>,
    mode: CaptureMode,
    selection: BufferSelection,
) -> String {
    let mode = encode_capture_mode(mode);
    let buffer = encode_optional_text(buffer);
    match selection {
        BufferSelection::All => format!("SAVE_BUFFER\t{session}\t{mode}\t{buffer}\n"),
        BufferSelection::LineRange { start, end } => {
            format!("SAVE_BUFFER_LINES\t{session}\t{mode}\t{buffer}\t{start}\t{end}\n")
        }
        BufferSelection::Search(needle) => {
            format!(
                "SAVE_BUFFER_SEARCH\t{session}\t{mode}\t{buffer}\t{}\n",
                encode_hex(needle.as_bytes())
            )
        }
    }
}

#[allow(dead_code)]
pub fn encode_save_buffer_text(session: &str, buffer: Option<&str>, text: &str) -> String {
    format!(
        "SAVE_BUFFER_TEXT\t{session}\t{}\t{}\n",
        encode_optional_text(buffer),
        encode_hex(text.as_bytes())
    )
}

pub fn encode_list_buffers() -> &'static str {
    "LIST_BUFFERS\n"
}

pub fn encode_copy_mode(session: &str, mode: CaptureMode, search: Option<&str>) -> String {
    format!(
        "COPY_MODE\t{session}\t{}\t{}\n",
        encode_capture_mode(mode),
        encode_optional_text(search)
    )
}

pub fn encode_paste_buffer(session: &str, buffer: Option<&str>) -> String {
    format!(
        "PASTE_BUFFER\t{session}\t{}\n",
        encode_optional_text(buffer)
    )
}

pub fn encode_delete_buffer(buffer: &str) -> String {
    format!("DELETE_BUFFER\t{}\n", encode_hex(buffer.as_bytes()))
}

pub fn encode_resize(session: &str, cols: u16, rows: u16) -> String {
    format!("RESIZE\t{session}\t{cols}\t{rows}\n")
}

pub fn encode_send(session: &str, bytes: &[u8]) -> String {
    format!("SEND\t{session}\t{}\n", encode_hex(bytes))
}

pub fn encode_split(session: &str, direction: SplitDirection, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "SPLIT\t{session}\t{}\t{}\t{joined}\n",
        encode_split_direction(direction),
        command.len()
    )
}

pub fn encode_list_panes(session: &str, format: Option<&str>) -> String {
    match format {
        Some(format) => format!(
            "LIST_PANES_FORMAT\t{session}\t{}\n",
            encode_hex(format.as_bytes())
        ),
        None => format!("LIST_PANES\t{session}\n"),
    }
}

pub fn encode_select_pane(session: &str, pane: usize) -> String {
    format!("SELECT_PANE\t{session}\t{pane}\n")
}

pub fn encode_kill_pane(session: &str, pane: Option<usize>) -> String {
    match pane {
        Some(pane) => format!("KILL_PANE\t{session}\t{pane}\n"),
        None => format!("KILL_PANE\t{session}\tactive\n"),
    }
}

pub fn encode_new_window(session: &str, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!("NEW_WINDOW\t{session}\t{}\t{joined}\n", command.len())
}

pub fn encode_list_windows(session: &str) -> String {
    format!("LIST_WINDOWS\t{session}\n")
}

pub fn encode_select_window(session: &str, window: usize) -> String {
    format!("SELECT_WINDOW\t{session}\t{window}\n")
}

pub fn encode_kill_window(session: &str, window: Option<usize>) -> String {
    match window {
        Some(window) => format!("KILL_WINDOW\t{session}\t{window}\n"),
        None => format!("KILL_WINDOW\t{session}\tactive\n"),
    }
}

pub fn encode_zoom_pane(session: &str, pane: Option<usize>) -> String {
    match pane {
        Some(pane) => format!("ZOOM_PANE\t{session}\t{pane}\n"),
        None => format!("ZOOM_PANE\t{session}\tactive\n"),
    }
}

pub fn encode_status_line(session: &str, format: Option<&str>) -> String {
    match format {
        Some(format) => format!(
            "STATUS_LINE_FORMAT\t{session}\t{}\n",
            encode_hex(format.as_bytes())
        ),
        None => format!("STATUS_LINE\t{session}\n"),
    }
}

pub fn encode_display_message(session: &str, format: &str) -> String {
    format!(
        "DISPLAY_MESSAGE\t{session}\t{}\n",
        encode_hex(format.as_bytes())
    )
}

pub fn encode_kill(session: &str) -> String {
    format!("KILL\t{session}\n")
}

pub fn encode_kill_server() -> &'static str {
    "KILL_SERVER\n"
}

pub fn decode_request(line: &str) -> Result<Request, String> {
    let line = line.trim_end_matches('\n');
    let parts = line.split('\t').collect::<Vec<_>>();

    match parts.as_slice() {
        ["NEW", session, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "NEW has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("NEW argc does not match command".to_string());
            }
            Ok(Request::New {
                session: (*session).to_string(),
                command,
            })
        }
        ["ATTACH", session] => Ok(Request::Attach {
            session: (*session).to_string(),
        }),
        ["ATTACH_RAW_STATE", session] => Ok(Request::AttachRawState {
            session: (*session).to_string(),
        }),
        ["LIST"] => Ok(Request::List),
        ["ATTACH_SNAPSHOT", session] => Ok(Request::AttachSnapshot {
            session: (*session).to_string(),
        }),
        ["ATTACH_LAYOUT_SNAPSHOT", session] => Ok(Request::AttachLayoutSnapshot {
            session: (*session).to_string(),
        }),
        ["ATTACH_LAYOUT_FRAME", session] => Ok(Request::AttachLayoutFrame {
            session: (*session).to_string(),
        }),
        ["ATTACH_EVENTS", session] => Ok(Request::AttachEvents {
            session: (*session).to_string(),
        }),
        ["ATTACH_RENDER", session] => Ok(Request::AttachRender {
            session: (*session).to_string(),
        }),
        ["CAPTURE", session] => Ok(Request::Capture {
            session: (*session).to_string(),
            mode: CaptureMode::All,
        }),
        ["CAPTURE", session, mode] => Ok(Request::Capture {
            session: (*session).to_string(),
            mode: decode_capture_mode(mode)?,
        }),
        ["SAVE_BUFFER", session, mode, buffer] => Ok(Request::SaveBuffer {
            session: (*session).to_string(),
            buffer: decode_optional_text(buffer, "SAVE_BUFFER")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::All,
        }),
        ["SAVE_BUFFER_LINES", session, mode, buffer, start, end] => Ok(Request::SaveBuffer {
            session: (*session).to_string(),
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_LINES")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::LineRange {
                start: decode_positive_index(start, "SAVE_BUFFER_LINES has invalid start line")?,
                end: decode_positive_index(end, "SAVE_BUFFER_LINES has invalid end line")?,
            },
        }),
        ["SAVE_BUFFER_SEARCH", session, mode, buffer, needle] => Ok(Request::SaveBuffer {
            session: (*session).to_string(),
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_SEARCH")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::Search(decode_utf8_hex(needle, "SAVE_BUFFER_SEARCH")?),
        }),
        ["SAVE_BUFFER_TEXT", session, buffer, text] => Ok(Request::SaveBufferText {
            session: (*session).to_string(),
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_TEXT")?,
            text: decode_utf8_hex_limited(
                text,
                "SAVE_BUFFER_TEXT",
                MAX_SAVE_BUFFER_TEXT_BYTES,
                "SAVE_BUFFER_TEXT exceeds maximum buffer size",
            )?,
        }),
        ["COPY_MODE", session, mode, search] => Ok(Request::CopyMode {
            session: (*session).to_string(),
            mode: decode_capture_mode(mode)?,
            search: decode_optional_text(search, "COPY_MODE")?,
        }),
        ["LIST_BUFFERS"] => Ok(Request::ListBuffers),
        ["PASTE_BUFFER", session, buffer] => Ok(Request::PasteBuffer {
            session: (*session).to_string(),
            buffer: decode_optional_text(buffer, "PASTE_BUFFER")?,
        }),
        ["DELETE_BUFFER", buffer] => Ok(Request::DeleteBuffer {
            buffer: decode_utf8_hex(buffer, "DELETE_BUFFER")?,
        }),
        ["RESIZE", session, cols, rows] => Ok(Request::Resize {
            session: (*session).to_string(),
            cols: cols
                .parse::<u16>()
                .map_err(|_| "RESIZE has invalid cols".to_string())?,
            rows: rows
                .parse::<u16>()
                .map_err(|_| "RESIZE has invalid rows".to_string())?,
        }),
        ["SEND", session, hex] => Ok(Request::Send {
            session: (*session).to_string(),
            bytes: decode_hex(hex)?,
        }),
        ["SPLIT", session, direction, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "SPLIT has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("SPLIT argc does not match command".to_string());
            }
            Ok(Request::Split {
                session: (*session).to_string(),
                direction: decode_split_direction(direction)?,
                command,
            })
        }
        ["LIST_PANES", session] => Ok(Request::ListPanes {
            session: (*session).to_string(),
            format: None,
        }),
        ["LIST_PANES_FORMAT", session, format] => Ok(Request::ListPanes {
            session: (*session).to_string(),
            format: Some(decode_utf8_hex(format, "LIST_PANES_FORMAT")?),
        }),
        ["SELECT_PANE", session, pane] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            pane: pane
                .parse::<usize>()
                .map_err(|_| "SELECT_PANE has invalid pane index".to_string())?,
        }),
        ["KILL_PANE", session, pane] => Ok(Request::KillPane {
            session: (*session).to_string(),
            pane: decode_optional_pane(pane)?,
        }),
        ["NEW_WINDOW", session, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "NEW_WINDOW has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("NEW_WINDOW argc does not match command".to_string());
            }
            Ok(Request::NewWindow {
                session: (*session).to_string(),
                command,
            })
        }
        ["LIST_WINDOWS", session] => Ok(Request::ListWindows {
            session: (*session).to_string(),
        }),
        ["SELECT_WINDOW", session, window] => Ok(Request::SelectWindow {
            session: (*session).to_string(),
            window: window
                .parse::<usize>()
                .map_err(|_| "SELECT_WINDOW has invalid window index".to_string())?,
        }),
        ["KILL_WINDOW", session, window] => Ok(Request::KillWindow {
            session: (*session).to_string(),
            window: decode_optional_window(window)?,
        }),
        ["ZOOM_PANE", session, pane] => Ok(Request::ZoomPane {
            session: (*session).to_string(),
            pane: decode_optional_zoom_pane(pane)?,
        }),
        ["STATUS_LINE", session] => Ok(Request::StatusLine {
            session: (*session).to_string(),
            format: None,
        }),
        ["STATUS_LINE_FORMAT", session, format] => Ok(Request::StatusLine {
            session: (*session).to_string(),
            format: Some(decode_utf8_hex(format, "STATUS_LINE_FORMAT")?),
        }),
        ["DISPLAY_MESSAGE", session, format] => Ok(Request::DisplayMessage {
            session: (*session).to_string(),
            format: decode_utf8_hex(format, "DISPLAY_MESSAGE")?,
        }),
        ["KILL", session] => Ok(Request::Kill {
            session: (*session).to_string(),
        }),
        ["KILL_SERVER"] => Ok(Request::KillServer),
        _ => Err(format!("unknown request line: {line:?}")),
    }
}

fn encode_split_direction(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Horizontal => "h",
        SplitDirection::Vertical => "v",
    }
}

fn encode_capture_mode(mode: CaptureMode) -> &'static str {
    match mode {
        CaptureMode::Screen => "screen",
        CaptureMode::History => "history",
        CaptureMode::All => "all",
    }
}

fn decode_capture_mode(value: &str) -> Result<CaptureMode, String> {
    match value {
        "screen" => Ok(CaptureMode::Screen),
        "history" => Ok(CaptureMode::History),
        "all" => Ok(CaptureMode::All),
        _ => Err("CAPTURE has invalid mode".to_string()),
    }
}

fn decode_split_direction(value: &str) -> Result<SplitDirection, String> {
    match value {
        "h" => Ok(SplitDirection::Horizontal),
        "v" => Ok(SplitDirection::Vertical),
        _ => Err("SPLIT has invalid direction".to_string()),
    }
}

fn decode_optional_pane(value: &str) -> Result<Option<usize>, String> {
    decode_optional_index(value, "KILL_PANE has invalid pane index")
}

fn decode_optional_zoom_pane(value: &str) -> Result<Option<usize>, String> {
    decode_optional_index(value, "ZOOM_PANE has invalid pane index")
}

fn decode_optional_window(value: &str) -> Result<Option<usize>, String> {
    decode_optional_index(value, "KILL_WINDOW has invalid window index")
}

fn decode_optional_index(value: &str, invalid_message: &str) -> Result<Option<usize>, String> {
    if value == "active" {
        Ok(None)
    } else {
        value
            .parse::<usize>()
            .map(Some)
            .map_err(|_| invalid_message.to_string())
    }
}

fn decode_positive_index(value: &str, invalid_message: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_message.to_string())
}

fn decode_utf8_hex(hex: &str, command: &str) -> Result<String, String> {
    String::from_utf8(decode_hex(hex)?).map_err(|_| format!("{command} has non-utf8 format"))
}

fn decode_utf8_hex_limited(
    hex: &str,
    command: &str,
    max_decoded_bytes: usize,
    too_large_message: &str,
) -> Result<String, String> {
    if hex.len() > max_decoded_bytes.saturating_mul(2) {
        return Err(too_large_message.to_string());
    }
    decode_utf8_hex(hex, command)
}

fn encode_optional_text(value: Option<&str>) -> String {
    value
        .map(|value| encode_hex(value.as_bytes()))
        .unwrap_or_default()
}

fn decode_optional_text(value: &str, command: &str) -> Result<Option<String>, String> {
    if value.is_empty() {
        Ok(None)
    } else {
        decode_utf8_hex(value, command).map(Some)
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("hex payload has odd length".to_string());
    }

    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let hi = hex_value(pair[0])?;
            let lo = hex_value(pair[1])?;
            Ok((hi << 4) | lo)
        })
        .collect()
}

fn hex_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("hex payload contains non-hex byte".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_new_request_with_spaced_args() {
        let command = vec!["sh".to_string(), "-c".to_string(), "echo ok".to_string()];
        let line = encode_new("dev", &command);

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::New {
                session: "dev".to_string(),
                command
            }
        );
    }

    #[test]
    fn round_trips_resize_request() {
        let line = encode_resize("dev", 100, 40);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Resize {
                session: "dev".to_string(),
                cols: 100,
                rows: 40,
            }
        );
    }

    #[test]
    fn round_trips_capture_screen_request() {
        let line = encode_capture("dev", CaptureMode::Screen);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Capture {
                session: "dev".to_string(),
                mode: CaptureMode::Screen,
            }
        );
    }

    #[test]
    fn round_trips_capture_history_request() {
        let line = encode_capture("dev", CaptureMode::History);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Capture {
                session: "dev".to_string(),
                mode: CaptureMode::History,
            }
        );
    }

    #[test]
    fn decodes_legacy_capture_request_as_all() {
        assert_eq!(
            decode_request("CAPTURE\tdev\n").unwrap(),
            Request::Capture {
                session: "dev".to_string(),
                mode: CaptureMode::All,
            }
        );
    }

    #[test]
    fn round_trips_save_buffer_request() {
        let line = encode_save_buffer(
            "dev",
            Some("saved"),
            CaptureMode::Screen,
            BufferSelection::All,
        );
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SaveBuffer {
                session: "dev".to_string(),
                buffer: Some("saved".to_string()),
                mode: CaptureMode::Screen,
                selection: BufferSelection::All,
            }
        );
    }

    #[test]
    fn round_trips_save_buffer_line_range_request() {
        let line = encode_save_buffer(
            "dev",
            Some("picked"),
            CaptureMode::Screen,
            BufferSelection::LineRange { start: 2, end: 3 },
        );
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SaveBuffer {
                session: "dev".to_string(),
                buffer: Some("picked".to_string()),
                mode: CaptureMode::Screen,
                selection: BufferSelection::LineRange { start: 2, end: 3 },
            }
        );
    }

    #[test]
    fn round_trips_save_buffer_search_request() {
        let line = encode_save_buffer(
            "dev",
            Some("match"),
            CaptureMode::All,
            BufferSelection::Search("needle".to_string()),
        );
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SaveBuffer {
                session: "dev".to_string(),
                buffer: Some("match".to_string()),
                mode: CaptureMode::All,
                selection: BufferSelection::Search("needle".to_string()),
            }
        );
    }

    #[test]
    fn round_trips_save_buffer_text_request() {
        let line = encode_save_buffer_text("dev", Some("picked"), "first\tline\nsecond line");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SaveBufferText {
                session: "dev".to_string(),
                buffer: Some("picked".to_string()),
                text: "first\tline\nsecond line".to_string(),
            }
        );
    }

    #[test]
    fn rejects_oversized_save_buffer_text_before_hex_decode() {
        let oversized = "zz".repeat(MAX_SAVE_BUFFER_TEXT_BYTES + 1);
        let line = format!("SAVE_BUFFER_TEXT\tdev\t\t{oversized}\n");

        assert_eq!(
            decode_request(&line).unwrap_err(),
            "SAVE_BUFFER_TEXT exceeds maximum buffer size"
        );
    }

    #[test]
    fn round_trips_copy_mode_request() {
        let line = encode_copy_mode("dev", CaptureMode::History, Some("needle"));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::CopyMode {
                session: "dev".to_string(),
                mode: CaptureMode::History,
                search: Some("needle".to_string()),
            }
        );
    }

    #[test]
    fn round_trips_paste_buffer_request() {
        let line = encode_paste_buffer("dev", Some("saved"));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::PasteBuffer {
                session: "dev".to_string(),
                buffer: Some("saved".to_string()),
            }
        );
    }

    #[test]
    fn round_trips_send_request() {
        let line = encode_send("dev", b"hello\r");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Send {
                session: "dev".to_string(),
                bytes: b"hello\r".to_vec(),
            }
        );
    }

    #[test]
    fn round_trips_split_request() {
        let command = vec!["sh".to_string(), "-c".to_string(), "echo split".to_string()];
        let line = encode_split("dev", SplitDirection::Horizontal, &command);

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Split {
                session: "dev".to_string(),
                direction: SplitDirection::Horizontal,
                command,
            }
        );
    }

    #[test]
    fn round_trips_select_pane_request() {
        let line = encode_select_pane("dev", 1);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SelectPane {
                session: "dev".to_string(),
                pane: 1,
            }
        );
    }

    #[test]
    fn round_trips_kill_pane_request_with_index() {
        let line = encode_kill_pane("dev", Some(1));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::KillPane {
                session: "dev".to_string(),
                pane: Some(1),
            }
        );
    }

    #[test]
    fn round_trips_kill_pane_request_for_active_pane() {
        let line = encode_kill_pane("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::KillPane {
                session: "dev".to_string(),
                pane: None,
            }
        );
    }

    #[test]
    fn round_trips_new_window_request() {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo window".to_string(),
        ];
        let line = encode_new_window("dev", &command);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::NewWindow {
                session: "dev".to_string(),
                command,
            }
        );
    }

    #[test]
    fn round_trips_select_window_request() {
        let line = encode_select_window("dev", 1);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SelectWindow {
                session: "dev".to_string(),
                window: 1,
            }
        );
    }

    #[test]
    fn round_trips_kill_window_request_with_index() {
        let line = encode_kill_window("dev", Some(1));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::KillWindow {
                session: "dev".to_string(),
                window: Some(1),
            }
        );
    }

    #[test]
    fn round_trips_kill_window_request_for_active_window() {
        let line = encode_kill_window("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::KillWindow {
                session: "dev".to_string(),
                window: None,
            }
        );
    }

    #[test]
    fn round_trips_zoom_pane_request_for_active_pane() {
        let line = encode_zoom_pane("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ZoomPane {
                session: "dev".to_string(),
                pane: None,
            }
        );
    }

    #[test]
    fn round_trips_zoom_pane_request_with_index() {
        let line = encode_zoom_pane("dev", Some(0));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ZoomPane {
                session: "dev".to_string(),
                pane: Some(0),
            }
        );
    }

    #[test]
    fn round_trips_list_panes_format_request() {
        let line = encode_list_panes("dev", Some("#{pane.index}:#{pane.zoomed}"));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ListPanes {
                session: "dev".to_string(),
                format: Some("#{pane.index}:#{pane.zoomed}".to_string()),
            }
        );
    }

    #[test]
    fn rejects_invalid_kill_pane_index_with_existing_message() {
        let err = decode_request("KILL_PANE\tdev\tbad\n").unwrap_err();
        assert_eq!(err, "KILL_PANE has invalid pane index");
    }

    #[test]
    fn rejects_invalid_kill_window_index() {
        let err = decode_request("KILL_WINDOW\tdev\tbad\n").unwrap_err();
        assert_eq!(err, "KILL_WINDOW has invalid window index");
    }

    #[test]
    fn rejects_invalid_zoom_pane_index() {
        let err = decode_request("ZOOM_PANE\tdev\tbad\n").unwrap_err();
        assert_eq!(err, "ZOOM_PANE has invalid pane index");
    }

    #[test]
    fn round_trips_status_line_request() {
        let line = encode_status_line("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::StatusLine {
                session: "dev".to_string(),
                format: None,
            }
        );
    }

    #[test]
    fn round_trips_attach_snapshot_request() {
        let line = encode_attach_snapshot("dev");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::AttachSnapshot {
                session: "dev".to_string(),
            }
        );
    }

    #[test]
    fn round_trips_attach_raw_state_request() {
        let line = encode_attach_raw_state("dev");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::AttachRawState {
                session: "dev".to_string(),
            }
        );
    }

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

    #[test]
    fn round_trips_attach_layout_frame_request() {
        let line = encode_attach_layout_frame("dev");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::AttachLayoutFrame {
                session: "dev".to_string(),
            }
        );
    }

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

    #[test]
    fn round_trips_attach_render_request() {
        let line = encode_attach_render("dev");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::AttachRender {
                session: "dev".to_string(),
            }
        );
    }

    #[test]
    fn round_trips_status_line_format_request() {
        let line = encode_status_line("dev", Some("#{session.name}:#{pane.index}"));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::StatusLine {
                session: "dev".to_string(),
                format: Some("#{session.name}:#{pane.index}".to_string()),
            }
        );
    }

    #[test]
    fn round_trips_display_message_request() {
        let line = encode_display_message("dev", "#{window.list}");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::DisplayMessage {
                session: "dev".to_string(),
                format: "#{window.list}".to_string(),
            }
        );
    }
}
