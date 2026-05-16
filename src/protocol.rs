pub const ARG_SEPARATOR: char = '\u{1f}';
pub const MAX_SAVE_BUFFER_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneResizeDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneSelectTarget {
    Index(usize),
    Id(usize),
    Direction(PaneDirection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowTarget {
    Active,
    Index(usize),
    Id(usize),
    Name(String),
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
    ListSessions {
        format: Option<String>,
    },
    RenameSession {
        old_name: String,
        new_name: String,
    },
    ListClients {
        session: Option<String>,
        format: Option<String>,
    },
    DetachClient {
        session: Option<String>,
        client_id: Option<usize>,
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
    ResizePane {
        session: String,
        direction: PaneResizeDirection,
        amount: usize,
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
        target: PaneSelectTarget,
    },
    KillPane {
        session: String,
        pane: Option<usize>,
    },
    RespawnPane {
        session: String,
        pane: Option<usize>,
        force: bool,
        command: Vec<String>,
    },
    NewWindow {
        session: String,
        command: Vec<String>,
    },
    ListWindows {
        session: String,
        format: Option<String>,
    },
    SelectWindow {
        session: String,
        target: WindowTarget,
    },
    RenameWindow {
        session: String,
        target: WindowTarget,
        name: String,
    },
    NextWindow {
        session: String,
    },
    PreviousWindow {
        session: String,
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

pub fn encode_list_sessions(format: Option<&str>) -> String {
    match format {
        Some(format) => format!("LIST_SESSIONS_FORMAT\t{}\n", encode_hex(format.as_bytes())),
        None => "LIST\n".to_string(),
    }
}

pub fn encode_rename_session(old_name: &str, new_name: &str) -> String {
    format!(
        "RENAME_SESSION\t{}\t{}\n",
        encode_hex(old_name.as_bytes()),
        encode_hex(new_name.as_bytes())
    )
}

pub fn encode_list_clients(session: Option<&str>, format: Option<&str>) -> String {
    format!(
        "LIST_CLIENTS\t{}\t{}\n",
        encode_optional_text(session),
        encode_optional_text(format)
    )
}

pub fn encode_detach_client(session: Option<&str>, client_id: Option<usize>) -> String {
    format!(
        "DETACH_CLIENT\t{}\t{}\n",
        encode_optional_text(session),
        client_id.map(|id| id.to_string()).unwrap_or_default()
    )
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

pub fn encode_resize_pane(session: &str, direction: PaneResizeDirection, amount: usize) -> String {
    format!(
        "RESIZE_PANE\t{session}\t{}\t{amount}\n",
        encode_pane_resize_direction(direction)
    )
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

pub fn encode_select_pane_target(session: &str, target: PaneSelectTarget) -> String {
    match target {
        PaneSelectTarget::Index(pane) => encode_select_pane(session, pane),
        PaneSelectTarget::Id(id) => format!("SELECT_PANE_ID\t{session}\t{id}\n"),
        PaneSelectTarget::Direction(direction) => {
            format!(
                "SELECT_PANE_DIRECTION\t{session}\t{}\n",
                encode_pane_direction(direction)
            )
        }
    }
}

pub fn encode_kill_pane(session: &str, pane: Option<usize>) -> String {
    match pane {
        Some(pane) => format!("KILL_PANE\t{session}\t{pane}\n"),
        None => format!("KILL_PANE\t{session}\tactive\n"),
    }
}

pub fn encode_respawn_pane(
    session: &str,
    pane: Option<usize>,
    force: bool,
    command: &[String],
) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "RESPAWN_PANE\t{session}\t{}\t{}\t{}\t{joined}\n",
        pane.map_or_else(|| "active".to_string(), |pane| pane.to_string()),
        usize::from(force),
        command.len()
    )
}

pub fn encode_new_window(session: &str, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!("NEW_WINDOW\t{session}\t{}\t{joined}\n", command.len())
}

pub fn encode_list_windows(session: &str, format: Option<&str>) -> String {
    match format {
        Some(format) => format!(
            "LIST_WINDOWS_FORMAT\t{session}\t{}\n",
            encode_hex(format.as_bytes())
        ),
        None => format!("LIST_WINDOWS\t{session}\n"),
    }
}

pub fn encode_select_window(session: &str, window: usize) -> String {
    format!("SELECT_WINDOW\t{session}\t{window}\n")
}

pub fn encode_select_window_target(session: &str, target: WindowTarget) -> String {
    match target {
        WindowTarget::Active => format!("SELECT_WINDOW_ACTIVE\t{session}\n"),
        WindowTarget::Index(index) => encode_select_window(session, index),
        WindowTarget::Id(id) => format!("SELECT_WINDOW_ID\t{session}\t{id}\n"),
        WindowTarget::Name(name) => {
            format!(
                "SELECT_WINDOW_NAME\t{session}\t{}\n",
                encode_hex(name.as_bytes())
            )
        }
    }
}

pub fn encode_rename_window(session: &str, target: WindowTarget, name: &str) -> String {
    let encoded_name = encode_hex(name.as_bytes());
    match target {
        WindowTarget::Active => format!("RENAME_WINDOW\t{session}\tactive\t{encoded_name}\n"),
        WindowTarget::Index(index) => {
            format!("RENAME_WINDOW\t{session}\t{index}\t{encoded_name}\n")
        }
        WindowTarget::Id(id) => format!("RENAME_WINDOW_ID\t{session}\t{id}\t{encoded_name}\n"),
        WindowTarget::Name(old_name) => format!(
            "RENAME_WINDOW_NAME\t{session}\t{}\t{encoded_name}\n",
            encode_hex(old_name.as_bytes())
        ),
    }
}

pub fn encode_next_window(session: &str) -> String {
    format!("NEXT_WINDOW\t{session}\n")
}

pub fn encode_previous_window(session: &str) -> String {
    format!("PREVIOUS_WINDOW\t{session}\n")
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
        ["LIST_SESSIONS"] => Ok(Request::ListSessions { format: None }),
        ["LIST_SESSIONS_FORMAT", format] => Ok(Request::ListSessions {
            format: Some(decode_utf8_hex(format, "LIST_SESSIONS_FORMAT")?),
        }),
        ["RENAME_SESSION", old_name, new_name] => Ok(Request::RenameSession {
            old_name: decode_utf8_hex(old_name, "RENAME_SESSION")?,
            new_name: decode_utf8_hex(new_name, "RENAME_SESSION")?,
        }),
        ["LIST_CLIENTS", session, format] => Ok(Request::ListClients {
            session: decode_optional_text(session, "LIST_CLIENTS")?,
            format: decode_optional_text(format, "LIST_CLIENTS")?,
        }),
        ["DETACH_CLIENT", session, client_id] => Ok(Request::DetachClient {
            session: decode_optional_text(session, "DETACH_CLIENT")?,
            client_id: decode_optional_client_id(client_id)?,
        }),
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
        ["RESIZE_PANE", session, direction, amount] => Ok(Request::ResizePane {
            session: (*session).to_string(),
            direction: decode_pane_resize_direction(direction)?,
            amount: amount
                .parse::<usize>()
                .ok()
                .filter(|amount| *amount > 0)
                .ok_or_else(|| "RESIZE_PANE amount must be a positive integer".to_string())?,
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
            target: PaneSelectTarget::Index(
                pane.parse::<usize>()
                    .map_err(|_| "SELECT_PANE has invalid pane index".to_string())?,
            ),
        }),
        ["SELECT_PANE_ID", session, id] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            target: PaneSelectTarget::Id(
                id.parse::<usize>()
                    .map_err(|_| "SELECT_PANE_ID has invalid pane id".to_string())?,
            ),
        }),
        ["SELECT_PANE_DIRECTION", session, direction] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            target: PaneSelectTarget::Direction(decode_pane_direction(
                direction,
                "SELECT_PANE_DIRECTION has invalid direction",
            )?),
        }),
        ["KILL_PANE", session, pane] => Ok(Request::KillPane {
            session: (*session).to_string(),
            pane: decode_optional_pane(pane)?,
        }),
        ["RESPAWN_PANE", session, pane, force, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "RESPAWN_PANE has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("RESPAWN_PANE argc does not match command".to_string());
            }
            Ok(Request::RespawnPane {
                session: (*session).to_string(),
                pane: decode_optional_pane(pane)?,
                force: match *force {
                    "0" => false,
                    "1" => true,
                    _ => return Err("RESPAWN_PANE has invalid force flag".to_string()),
                },
                command,
            })
        }
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
            format: None,
        }),
        ["LIST_WINDOWS_FORMAT", session, format] => Ok(Request::ListWindows {
            session: (*session).to_string(),
            format: Some(decode_utf8_hex(format, "LIST_WINDOWS_FORMAT")?),
        }),
        ["SELECT_WINDOW", session, window] => Ok(Request::SelectWindow {
            session: (*session).to_string(),
            target: WindowTarget::Index(
                window
                    .parse::<usize>()
                    .map_err(|_| "SELECT_WINDOW has invalid window index".to_string())?,
            ),
        }),
        ["SELECT_WINDOW_ID", session, id] => Ok(Request::SelectWindow {
            session: (*session).to_string(),
            target: WindowTarget::Id(
                id.parse::<usize>()
                    .map_err(|_| "SELECT_WINDOW_ID has invalid window id".to_string())?,
            ),
        }),
        ["SELECT_WINDOW_NAME", session, name] => Ok(Request::SelectWindow {
            session: (*session).to_string(),
            target: WindowTarget::Name(decode_utf8_hex(name, "SELECT_WINDOW_NAME")?),
        }),
        ["SELECT_WINDOW_ACTIVE", session] => Ok(Request::SelectWindow {
            session: (*session).to_string(),
            target: WindowTarget::Active,
        }),
        ["RENAME_WINDOW", session, target, name] => Ok(Request::RenameWindow {
            session: (*session).to_string(),
            target: decode_window_target(target, "RENAME_WINDOW has invalid window index")?,
            name: decode_utf8_hex(name, "RENAME_WINDOW")?,
        }),
        ["RENAME_WINDOW_ID", session, id, name] => Ok(Request::RenameWindow {
            session: (*session).to_string(),
            target: WindowTarget::Id(
                id.parse::<usize>()
                    .map_err(|_| "RENAME_WINDOW_ID has invalid window id".to_string())?,
            ),
            name: decode_utf8_hex(name, "RENAME_WINDOW_ID")?,
        }),
        ["RENAME_WINDOW_NAME", session, old_name, name] => Ok(Request::RenameWindow {
            session: (*session).to_string(),
            target: WindowTarget::Name(decode_utf8_hex(old_name, "RENAME_WINDOW_NAME")?),
            name: decode_utf8_hex(name, "RENAME_WINDOW_NAME")?,
        }),
        ["NEXT_WINDOW", session] => Ok(Request::NextWindow {
            session: (*session).to_string(),
        }),
        ["PREVIOUS_WINDOW", session] => Ok(Request::PreviousWindow {
            session: (*session).to_string(),
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

fn encode_pane_resize_direction(direction: PaneResizeDirection) -> &'static str {
    match direction {
        PaneResizeDirection::Left => "L",
        PaneResizeDirection::Right => "R",
        PaneResizeDirection::Up => "U",
        PaneResizeDirection::Down => "D",
    }
}

fn encode_pane_direction(direction: PaneDirection) -> &'static str {
    match direction {
        PaneDirection::Left => "L",
        PaneDirection::Right => "R",
        PaneDirection::Up => "U",
        PaneDirection::Down => "D",
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

fn decode_pane_resize_direction(value: &str) -> Result<PaneResizeDirection, String> {
    match value {
        "L" => Ok(PaneResizeDirection::Left),
        "R" => Ok(PaneResizeDirection::Right),
        "U" => Ok(PaneResizeDirection::Up),
        "D" => Ok(PaneResizeDirection::Down),
        _ => Err("RESIZE_PANE has invalid direction".to_string()),
    }
}

fn decode_pane_direction(value: &str, invalid_message: &str) -> Result<PaneDirection, String> {
    match value {
        "L" => Ok(PaneDirection::Left),
        "R" => Ok(PaneDirection::Right),
        "U" => Ok(PaneDirection::Up),
        "D" => Ok(PaneDirection::Down),
        _ => Err(invalid_message.to_string()),
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

fn decode_optional_client_id(value: &str) -> Result<Option<usize>, String> {
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse::<usize>()
            .map(Some)
            .map_err(|_| "DETACH_CLIENT has invalid client id".to_string())
    }
}

fn decode_window_target(value: &str, invalid_message: &str) -> Result<WindowTarget, String> {
    if value == "active" {
        Ok(WindowTarget::Active)
    } else {
        value
            .parse::<usize>()
            .map(WindowTarget::Index)
            .map_err(|_| invalid_message.to_string())
    }
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
    fn round_trips_session_lifecycle_requests() {
        assert_eq!(
            decode_request("LIST_SESSIONS\n").unwrap(),
            Request::ListSessions { format: None }
        );
        assert_eq!(
            decode_request(&encode_list_sessions(Some("#{session.name}"))).unwrap(),
            Request::ListSessions {
                format: Some("#{session.name}".to_string()),
            }
        );
        assert_eq!(
            decode_request(&encode_rename_session("old", "new")).unwrap(),
            Request::RenameSession {
                old_name: "old".to_string(),
                new_name: "new".to_string(),
            }
        );
        assert_eq!(
            decode_request(&encode_list_clients(Some("dev"), Some("#{client.id}"))).unwrap(),
            Request::ListClients {
                session: Some("dev".to_string()),
                format: Some("#{client.id}".to_string()),
            }
        );
        assert_eq!(
            decode_request(&encode_detach_client(Some("dev"), Some(7))).unwrap(),
            Request::DetachClient {
                session: Some("dev".to_string()),
                client_id: Some(7),
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
    fn round_trips_directional_resize_pane_request() {
        let line = encode_resize_pane("dev", PaneResizeDirection::Left, 5);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ResizePane {
                session: "dev".to_string(),
                direction: PaneResizeDirection::Left,
                amount: 5,
            }
        );
    }

    #[test]
    fn rejects_zero_directional_resize_pane_amount() {
        let err = decode_request("RESIZE_PANE\tdev\tL\t0\n").unwrap_err();
        assert!(err.contains("positive"), "{err}");
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
                target: PaneSelectTarget::Index(1),
            }
        );
    }

    #[test]
    fn round_trips_select_pane_id_request() {
        let line = encode_select_pane_target("dev", PaneSelectTarget::Id(42));
        assert_eq!(line, "SELECT_PANE_ID\tdev\t42\n");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SelectPane {
                session: "dev".to_string(),
                target: PaneSelectTarget::Id(42),
            }
        );
    }

    #[test]
    fn round_trips_select_pane_direction_request() {
        let line =
            encode_select_pane_target("dev", PaneSelectTarget::Direction(PaneDirection::Left));
        assert_eq!(line, "SELECT_PANE_DIRECTION\tdev\tL\n");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SelectPane {
                session: "dev".to_string(),
                target: PaneSelectTarget::Direction(PaneDirection::Left),
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
    fn round_trips_respawn_pane_request() {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf ready".to_string(),
        ];
        let line = encode_respawn_pane("dev", Some(1), true, &command);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::RespawnPane {
                session: "dev".to_string(),
                pane: Some(1),
                force: true,
                command,
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
                target: WindowTarget::Index(1),
            }
        );
    }

    #[test]
    fn round_trips_select_window_by_id_and_name_requests() {
        assert_eq!(
            decode_request(&encode_select_window_target("dev", WindowTarget::Id(42))).unwrap(),
            Request::SelectWindow {
                session: "dev".to_string(),
                target: WindowTarget::Id(42),
            }
        );
        assert_eq!(
            decode_request(&encode_select_window_target(
                "dev",
                WindowTarget::Name("editor".to_string())
            ))
            .unwrap(),
            Request::SelectWindow {
                session: "dev".to_string(),
                target: WindowTarget::Name("editor".to_string()),
            }
        );
    }

    #[test]
    fn round_trips_rename_and_cycle_window_requests() {
        assert_eq!(
            decode_request(&encode_rename_window("dev", WindowTarget::Active, "editor")).unwrap(),
            Request::RenameWindow {
                session: "dev".to_string(),
                target: WindowTarget::Active,
                name: "editor".to_string(),
            }
        );
        assert_eq!(
            decode_request(&encode_next_window("dev")).unwrap(),
            Request::NextWindow {
                session: "dev".to_string(),
            }
        );
        assert_eq!(
            decode_request(&encode_previous_window("dev")).unwrap(),
            Request::PreviousWindow {
                session: "dev".to_string(),
            }
        );
    }

    #[test]
    fn round_trips_list_windows_format_and_rename_targets() {
        assert_eq!(
            decode_request(&encode_list_windows("dev", None)).unwrap(),
            Request::ListWindows {
                session: "dev".to_string(),
                format: None,
            }
        );
        assert_eq!(
            decode_request(&encode_list_windows(
                "dev",
                Some("#{window.id}:#{window.name}")
            ))
            .unwrap(),
            Request::ListWindows {
                session: "dev".to_string(),
                format: Some("#{window.id}:#{window.name}".to_string()),
            }
        );
        assert_eq!(
            decode_request(&encode_rename_window("dev", WindowTarget::Id(7), "logs")).unwrap(),
            Request::RenameWindow {
                session: "dev".to_string(),
                target: WindowTarget::Id(7),
                name: "logs".to_string(),
            }
        );
        assert_eq!(
            decode_request(&encode_rename_window(
                "dev",
                WindowTarget::Name("old".to_string()),
                "new"
            ))
            .unwrap(),
            Request::RenameWindow {
                session: "dev".to_string(),
                target: WindowTarget::Name("old".to_string()),
                name: "new".to_string(),
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
