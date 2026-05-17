use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

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
pub enum LayoutPreset {
    EvenHorizontal,
    EvenVertical,
    Tiled,
    MainHorizontal,
    MainVertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneSelectTarget {
    Index(usize),
    Id(usize),
    Direction(PaneDirection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneTarget {
    Active,
    Index(usize),
    Id(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowTarget {
    Active,
    Index(usize),
    Id(usize),
    Name(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub session: String,
    pub window: WindowTarget,
    pub pane: PaneTarget,
}

impl Target {
    pub fn active(session: String) -> Self {
        Self {
            session,
            window: WindowTarget::Active,
            pane: PaneTarget::Active,
        }
    }
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
    LineRange { start: isize, end: isize },
    Search { needle: String, match_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    pub key: String,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionEntry {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    New {
        session: String,
        command: Vec<String>,
        cwd: Option<PathBuf>,
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
        target: Target,
        mode: CaptureMode,
        selection: BufferSelection,
    },
    SaveBuffer {
        target: Target,
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
        match_index: Option<usize>,
    },
    ListBuffers {
        format: Option<String>,
    },
    PasteBuffer {
        target: Target,
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
        target: Target,
        direction: PaneResizeDirection,
        amount: usize,
    },
    SelectLayout {
        session: String,
        window: WindowTarget,
        preset: LayoutPreset,
    },
    Send {
        target: Target,
        bytes: Vec<u8>,
    },
    Split {
        target: Target,
        direction: SplitDirection,
        command: Vec<String>,
        cwd: Option<PathBuf>,
    },
    ListPanes {
        session: String,
        window: WindowTarget,
        format: Option<String>,
    },
    SelectPane {
        session: String,
        window: WindowTarget,
        target: PaneSelectTarget,
    },
    KillPane {
        target: Target,
    },
    SwapPane {
        source: Target,
        destination: Target,
    },
    MovePane {
        source: Target,
        destination: Target,
        direction: SplitDirection,
    },
    BreakPane {
        target: Target,
    },
    JoinPane {
        source: Target,
        destination: Target,
        direction: SplitDirection,
    },
    RespawnPane {
        target: Target,
        force: bool,
        command: Vec<String>,
        cwd: Option<PathBuf>,
    },
    NewWindow {
        session: String,
        command: Vec<String>,
        cwd: Option<PathBuf>,
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
        target: WindowTarget,
    },
    ZoomPane {
        target: Target,
    },
    StatusLine {
        session: String,
        format: Option<String>,
    },
    DisplayMessage {
        session: String,
        format: String,
    },
    ListKeys {
        format: Option<String>,
    },
    BindKey {
        key: String,
        command: String,
    },
    UnbindKey {
        key: String,
    },
    ShowOptions {
        format: Option<String>,
    },
    SetOption {
        name: String,
        value: String,
    },
    Kill {
        session: String,
    },
    KillServer,
}

#[allow(dead_code)]
pub fn encode_new(session: &str, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!("NEW\t{session}\t{}\t{joined}\n", command.len())
}

pub fn encode_new_in_cwd(session: &str, command: &[String], cwd: &Path) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "NEW_CWD\t{session}\t{}\t{}\t{joined}\n",
        encode_path(cwd),
        command.len()
    )
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

#[allow(dead_code)]
pub fn encode_capture(session: &str, mode: CaptureMode) -> String {
    format!("CAPTURE\t{session}\t{}\n", encode_capture_mode(mode))
}

#[allow(dead_code)]
pub fn encode_capture_with_selection(
    session: &str,
    mode: CaptureMode,
    selection: BufferSelection,
) -> String {
    encode_capture_target(&Target::active(session.to_string()), mode, selection)
}

pub fn encode_capture_target(
    target: &Target,
    mode: CaptureMode,
    selection: BufferSelection,
) -> String {
    let mode = encode_capture_mode(mode);
    let target = encode_target(target);
    match selection {
        BufferSelection::All => format!("CAPTURE_TARGET\t{target}\t{mode}\n"),
        BufferSelection::LineRange { start, end } => {
            format!("CAPTURE_TARGET_LINES\t{target}\t{mode}\t{start}\t{end}\n")
        }
        BufferSelection::Search {
            needle,
            match_index,
        } => {
            format!(
                "CAPTURE_TARGET_SEARCH\t{target}\t{mode}\t{}\t{match_index}\n",
                encode_hex(needle.as_bytes())
            )
        }
    }
}

#[allow(dead_code)]
pub fn encode_save_buffer(
    session: &str,
    buffer: Option<&str>,
    mode: CaptureMode,
    selection: BufferSelection,
) -> String {
    encode_save_buffer_target(
        &Target::active(session.to_string()),
        buffer,
        mode,
        selection,
    )
}

pub fn encode_save_buffer_target(
    target: &Target,
    buffer: Option<&str>,
    mode: CaptureMode,
    selection: BufferSelection,
) -> String {
    let mode = encode_capture_mode(mode);
    let buffer = encode_optional_text(buffer);
    let target = encode_target(target);
    match selection {
        BufferSelection::All => format!("SAVE_BUFFER_TARGET\t{target}\t{mode}\t{buffer}\n"),
        BufferSelection::LineRange { start, end } => {
            format!("SAVE_BUFFER_TARGET_LINES\t{target}\t{mode}\t{buffer}\t{start}\t{end}\n")
        }
        BufferSelection::Search {
            needle,
            match_index,
        } => {
            let needle = encode_hex(needle.as_bytes());
            if match_index == 1 {
                format!("SAVE_BUFFER_TARGET_SEARCH\t{target}\t{mode}\t{buffer}\t{needle}\n")
            } else {
                format!(
                    "SAVE_BUFFER_TARGET_SEARCH\t{target}\t{mode}\t{buffer}\t{needle}\t{match_index}\n"
                )
            }
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

pub fn encode_list_buffers(format: Option<&str>) -> String {
    match format {
        Some(format) => format!("LIST_BUFFERS_FORMAT\t{}\n", encode_hex(format.as_bytes())),
        None => "LIST_BUFFERS\n".to_string(),
    }
}

pub fn encode_copy_mode(
    session: &str,
    mode: CaptureMode,
    search: Option<&str>,
    match_index: Option<usize>,
) -> String {
    let mode = encode_capture_mode(mode);
    let search = encode_optional_text(search);
    match match_index {
        Some(match_index) => format!("COPY_MODE\t{session}\t{mode}\t{search}\t{match_index}\n"),
        None => format!("COPY_MODE\t{session}\t{mode}\t{search}\n"),
    }
}

#[allow(dead_code)]
pub fn encode_paste_buffer(session: &str, buffer: Option<&str>) -> String {
    encode_paste_buffer_target(&Target::active(session.to_string()), buffer)
}

pub fn encode_paste_buffer_target(target: &Target, buffer: Option<&str>) -> String {
    format!(
        "PASTE_BUFFER_TARGET\t{}\t{}\n",
        encode_target(target),
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
    encode_resize_pane_target(&Target::active(session.to_string()), direction, amount)
}

pub fn encode_resize_pane_target(
    target: &Target,
    direction: PaneResizeDirection,
    amount: usize,
) -> String {
    format!(
        "RESIZE_PANE_TARGET\t{}\t{}\t{amount}\n",
        encode_target(target),
        encode_pane_resize_direction(direction)
    )
}

pub fn encode_select_layout(session: &str, preset: LayoutPreset) -> String {
    encode_select_layout_target(&Target::active(session.to_string()), preset)
}

pub fn encode_select_layout_target(target: &Target, preset: LayoutPreset) -> String {
    format!(
        "SELECT_LAYOUT_TARGET\t{}\t{}\n",
        encode_target(target),
        encode_layout_preset(preset)
    )
}

#[allow(dead_code)]
pub fn encode_send(session: &str, bytes: &[u8]) -> String {
    encode_send_target(&Target::active(session.to_string()), bytes)
}

pub fn encode_send_target(target: &Target, bytes: &[u8]) -> String {
    format!(
        "SEND_TARGET\t{}\t{}\n",
        encode_target(target),
        encode_hex(bytes)
    )
}

pub fn encode_split(session: &str, direction: SplitDirection, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "SPLIT\t{session}\t{}\t{}\t{joined}\n",
        encode_split_direction(direction),
        command.len()
    )
}

#[allow(dead_code)]
pub fn encode_split_target(
    target: &Target,
    direction: SplitDirection,
    command: &[String],
) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "SPLIT_TARGET\t{}\t{}\t{}\t{joined}\n",
        encode_target(target),
        encode_split_direction(direction),
        command.len()
    )
}

pub fn encode_split_target_in_cwd(
    target: &Target,
    direction: SplitDirection,
    command: &[String],
    cwd: &Path,
) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "SPLIT_TARGET_CWD\t{}\t{}\t{}\t{}\t{joined}\n",
        encode_target(target),
        encode_split_direction(direction),
        encode_path(cwd),
        command.len()
    )
}

pub fn encode_list_panes(session: &str, format: Option<&str>) -> String {
    encode_list_panes_target(session, WindowTarget::Active, format)
}

pub fn encode_list_panes_target(
    session: &str,
    window: WindowTarget,
    format: Option<&str>,
) -> String {
    let window = encode_window_target(window);
    match format {
        Some(format) => format!(
            "LIST_PANES_TARGET_FORMAT\t{session}\t{window}\t{}\n",
            encode_hex(format.as_bytes())
        ),
        None => format!("LIST_PANES_TARGET\t{session}\t{window}\n"),
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

pub fn encode_select_pane_in_window(
    session: &str,
    window: WindowTarget,
    target: PaneSelectTarget,
) -> String {
    let window = encode_window_target(window);
    match target {
        PaneSelectTarget::Index(pane) => {
            format!("SELECT_PANE_TARGET\t{session}\t{window}\t{pane}\n")
        }
        PaneSelectTarget::Id(id) => format!("SELECT_PANE_TARGET_ID\t{session}\t{window}\t{id}\n"),
        PaneSelectTarget::Direction(direction) => {
            format!(
                "SELECT_PANE_TARGET_DIRECTION\t{session}\t{window}\t{}\n",
                encode_pane_direction(direction)
            )
        }
    }
}

pub fn encode_kill_pane(session: &str, pane: Option<usize>) -> String {
    let mut target = Target::active(session.to_string());
    target.pane = pane.map_or(PaneTarget::Active, PaneTarget::Index);
    encode_kill_pane_target(&target)
}

pub fn encode_kill_pane_target(target: &Target) -> String {
    format!("KILL_PANE_TARGET\t{}\n", encode_target(target))
}

pub fn encode_swap_pane(source: &Target, destination: &Target) -> String {
    format!(
        "SWAP_PANE\t{}\t{}\n",
        encode_target(source),
        encode_target(destination)
    )
}

pub fn encode_move_pane(
    source: &Target,
    destination: &Target,
    direction: SplitDirection,
) -> String {
    format!(
        "MOVE_PANE\t{}\t{}\t{}\n",
        encode_target(source),
        encode_target(destination),
        encode_split_direction(direction)
    )
}

pub fn encode_break_pane(target: &Target) -> String {
    format!("BREAK_PANE\t{}\n", encode_target(target))
}

pub fn encode_join_pane(
    source: &Target,
    destination: &Target,
    direction: SplitDirection,
) -> String {
    format!(
        "JOIN_PANE\t{}\t{}\t{}\n",
        encode_target(source),
        encode_target(destination),
        encode_split_direction(direction)
    )
}

#[allow(dead_code)]
pub fn encode_respawn_pane(
    session: &str,
    pane: Option<usize>,
    force: bool,
    command: &[String],
) -> String {
    let mut target = Target::active(session.to_string());
    target.pane = pane.map_or(PaneTarget::Active, PaneTarget::Index);
    encode_respawn_pane_target(&target, force, command)
}

pub fn encode_respawn_pane_target(target: &Target, force: bool, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "RESPAWN_PANE_TARGET\t{}\t{}\t{}\t{joined}\n",
        encode_target(target),
        usize::from(force),
        command.len()
    )
}

pub fn encode_respawn_pane_target_in_cwd(
    target: &Target,
    force: bool,
    command: &[String],
    cwd: &Path,
) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "RESPAWN_PANE_TARGET_CWD\t{}\t{}\t{}\t{}\t{joined}\n",
        encode_target(target),
        usize::from(force),
        encode_path(cwd),
        command.len()
    )
}

pub fn encode_new_window(session: &str, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!("NEW_WINDOW\t{session}\t{}\t{joined}\n", command.len())
}

pub fn encode_new_window_in_cwd(session: &str, command: &[String], cwd: &Path) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "NEW_WINDOW_CWD\t{session}\t{}\t{}\t{joined}\n",
        encode_path(cwd),
        command.len()
    )
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

#[allow(dead_code)]
pub fn encode_kill_window(session: &str, window: Option<usize>) -> String {
    encode_kill_window_target(
        session,
        window.map_or(WindowTarget::Active, WindowTarget::Index),
    )
}

pub fn encode_kill_window_target(session: &str, target: WindowTarget) -> String {
    format!(
        "KILL_WINDOW_TARGET\t{session}\t{}\n",
        encode_window_target(target)
    )
}

pub fn encode_zoom_pane(session: &str, pane: Option<usize>) -> String {
    let mut target = Target::active(session.to_string());
    target.pane = pane.map_or(PaneTarget::Active, PaneTarget::Index);
    encode_zoom_pane_target(&target)
}

pub fn encode_zoom_pane_target(target: &Target) -> String {
    format!("ZOOM_PANE_TARGET\t{}\n", encode_target(target))
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

pub fn encode_list_keys(format: Option<&str>) -> String {
    format!(
        "LIST_KEYS\t{}\n",
        format
            .map(|value| encode_hex(value.as_bytes()))
            .unwrap_or_default()
    )
}

pub fn encode_bind_key(key: &str, command: &str) -> String {
    format!(
        "BIND_KEY\t{}\t{}\n",
        encode_hex(key.as_bytes()),
        encode_hex(command.as_bytes())
    )
}

pub fn encode_unbind_key(key: &str) -> String {
    format!("UNBIND_KEY\t{}\n", encode_hex(key.as_bytes()))
}

pub fn encode_show_options(format: Option<&str>) -> String {
    format!(
        "SHOW_OPTIONS\t{}\n",
        format
            .map(|value| encode_hex(value.as_bytes()))
            .unwrap_or_default()
    )
}

pub fn encode_set_option(name: &str, value: &str) -> String {
    format!(
        "SET_OPTION\t{}\t{}\n",
        encode_hex(name.as_bytes()),
        encode_hex(value.as_bytes())
    )
}

pub fn encode_kill(session: &str) -> String {
    format!("KILL\t{session}\n")
}

pub fn encode_kill_server() -> &'static str {
    "KILL_SERVER\n"
}

pub fn encode_target(target: &Target) -> String {
    format!(
        "{}|{}|{}",
        encode_hex(target.session.as_bytes()),
        encode_window_target(target.window.clone()),
        encode_pane_target(&target.pane)
    )
}

fn encode_window_target(target: WindowTarget) -> String {
    match target {
        WindowTarget::Active => "active".to_string(),
        WindowTarget::Index(index) => format!("index:{index}"),
        WindowTarget::Id(id) => format!("id:{id}"),
        WindowTarget::Name(name) => format!("name:{}", encode_hex(name.as_bytes())),
    }
}

fn encode_pane_target(target: &PaneTarget) -> String {
    match target {
        PaneTarget::Active => "active".to_string(),
        PaneTarget::Index(index) => format!("index:{index}"),
        PaneTarget::Id(id) => format!("id:{id}"),
    }
}

fn decode_target(value: &str, command: &str) -> Result<Target, String> {
    let parts = value.split('|').collect::<Vec<_>>();
    let [session, window, pane] = parts.as_slice() else {
        return Err(format!("{command} has invalid target"));
    };
    Ok(Target {
        session: decode_utf8_hex(session, command)?,
        window: decode_structured_window_target(window, command)?,
        pane: decode_structured_pane_target(pane, command)?,
    })
}

fn decode_structured_window_target(value: &str, command: &str) -> Result<WindowTarget, String> {
    if value == "active" {
        return Ok(WindowTarget::Active);
    }
    let Some((kind, value)) = value.split_once(':') else {
        return Err(format!("{command} has invalid window target"));
    };
    match kind {
        "index" => value
            .parse::<usize>()
            .map(WindowTarget::Index)
            .map_err(|_| format!("{command} has invalid window index")),
        "id" => value
            .parse::<usize>()
            .map(WindowTarget::Id)
            .map_err(|_| format!("{command} has invalid window id")),
        "name" => decode_utf8_hex(value, command).map(WindowTarget::Name),
        _ => Err(format!("{command} has invalid window target")),
    }
}

fn decode_structured_pane_target(value: &str, command: &str) -> Result<PaneTarget, String> {
    if value == "active" {
        return Ok(PaneTarget::Active);
    }
    let Some((kind, value)) = value.split_once(':') else {
        return Err(format!("{command} has invalid pane target"));
    };
    match kind {
        "index" => value
            .parse::<usize>()
            .map(PaneTarget::Index)
            .map_err(|_| format!("{command} has invalid pane index")),
        "id" => value
            .parse::<usize>()
            .map(PaneTarget::Id)
            .map_err(|_| format!("{command} has invalid pane id")),
        _ => Err(format!("{command} has invalid pane target")),
    }
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
                cwd: None,
            })
        }
        ["NEW_CWD", session, cwd, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "NEW_CWD has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("NEW_CWD argc does not match command".to_string());
            }
            Ok(Request::New {
                session: (*session).to_string(),
                command,
                cwd: Some(decode_path(cwd)?),
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
            target: Target::active((*session).to_string()),
            mode: CaptureMode::All,
            selection: BufferSelection::All,
        }),
        ["CAPTURE", session, mode] => Ok(Request::Capture {
            target: Target::active((*session).to_string()),
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::All,
        }),
        ["CAPTURE_LINES", session, mode, start, end] => Ok(Request::Capture {
            target: Target::active((*session).to_string()),
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::LineRange {
                start: decode_line_offset(start, "CAPTURE_LINES has invalid start line")?,
                end: decode_line_offset(end, "CAPTURE_LINES has invalid end line")?,
            },
        }),
        ["CAPTURE_SEARCH", session, mode, needle, match_index] => Ok(Request::Capture {
            target: Target::active((*session).to_string()),
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::Search {
                needle: decode_utf8_hex(needle, "CAPTURE_SEARCH")?,
                match_index: decode_positive_index(
                    match_index,
                    "CAPTURE_SEARCH has invalid match index",
                )?,
            },
        }),
        ["CAPTURE_TARGET", target, mode] => Ok(Request::Capture {
            target: decode_target(target, "CAPTURE_TARGET")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::All,
        }),
        ["CAPTURE_TARGET_LINES", target, mode, start, end] => Ok(Request::Capture {
            target: decode_target(target, "CAPTURE_TARGET_LINES")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::LineRange {
                start: decode_line_offset(start, "CAPTURE_TARGET_LINES has invalid start line")?,
                end: decode_line_offset(end, "CAPTURE_TARGET_LINES has invalid end line")?,
            },
        }),
        ["CAPTURE_TARGET_SEARCH", target, mode, needle, match_index] => Ok(Request::Capture {
            target: decode_target(target, "CAPTURE_TARGET_SEARCH")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::Search {
                needle: decode_utf8_hex(needle, "CAPTURE_TARGET_SEARCH")?,
                match_index: decode_positive_index(
                    match_index,
                    "CAPTURE_TARGET_SEARCH has invalid match index",
                )?,
            },
        }),
        ["SAVE_BUFFER", session, mode, buffer] => Ok(Request::SaveBuffer {
            target: Target::active((*session).to_string()),
            buffer: decode_optional_text(buffer, "SAVE_BUFFER")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::All,
        }),
        ["SAVE_BUFFER_LINES", session, mode, buffer, start, end] => Ok(Request::SaveBuffer {
            target: Target::active((*session).to_string()),
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_LINES")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::LineRange {
                start: decode_line_offset(start, "SAVE_BUFFER_LINES has invalid start line")?,
                end: decode_line_offset(end, "SAVE_BUFFER_LINES has invalid end line")?,
            },
        }),
        ["SAVE_BUFFER_SEARCH", session, mode, buffer, needle] => Ok(Request::SaveBuffer {
            target: Target::active((*session).to_string()),
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_SEARCH")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::Search {
                needle: decode_utf8_hex(needle, "SAVE_BUFFER_SEARCH")?,
                match_index: 1,
            },
        }),
        [
            "SAVE_BUFFER_SEARCH",
            session,
            mode,
            buffer,
            needle,
            match_index,
        ] => Ok(Request::SaveBuffer {
            target: Target::active((*session).to_string()),
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_SEARCH")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::Search {
                needle: decode_utf8_hex(needle, "SAVE_BUFFER_SEARCH")?,
                match_index: decode_positive_index(
                    match_index,
                    "SAVE_BUFFER_SEARCH has invalid match index",
                )?,
            },
        }),
        ["SAVE_BUFFER_TARGET", target, mode, buffer] => Ok(Request::SaveBuffer {
            target: decode_target(target, "SAVE_BUFFER_TARGET")?,
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_TARGET")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::All,
        }),
        ["SAVE_BUFFER_TARGET_LINES", target, mode, buffer, start, end] => Ok(Request::SaveBuffer {
            target: decode_target(target, "SAVE_BUFFER_TARGET_LINES")?,
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_TARGET_LINES")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::LineRange {
                start: decode_line_offset(
                    start,
                    "SAVE_BUFFER_TARGET_LINES has invalid start line",
                )?,
                end: decode_line_offset(end, "SAVE_BUFFER_TARGET_LINES has invalid end line")?,
            },
        }),
        ["SAVE_BUFFER_TARGET_SEARCH", target, mode, buffer, needle] => Ok(Request::SaveBuffer {
            target: decode_target(target, "SAVE_BUFFER_TARGET_SEARCH")?,
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_TARGET_SEARCH")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::Search {
                needle: decode_utf8_hex(needle, "SAVE_BUFFER_TARGET_SEARCH")?,
                match_index: 1,
            },
        }),
        [
            "SAVE_BUFFER_TARGET_SEARCH",
            target,
            mode,
            buffer,
            needle,
            match_index,
        ] => Ok(Request::SaveBuffer {
            target: decode_target(target, "SAVE_BUFFER_TARGET_SEARCH")?,
            buffer: decode_optional_text(buffer, "SAVE_BUFFER_TARGET_SEARCH")?,
            mode: decode_capture_mode(mode)?,
            selection: BufferSelection::Search {
                needle: decode_utf8_hex(needle, "SAVE_BUFFER_TARGET_SEARCH")?,
                match_index: decode_positive_index(
                    match_index,
                    "SAVE_BUFFER_TARGET_SEARCH has invalid match index",
                )?,
            },
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
            match_index: None,
        }),
        ["COPY_MODE", session, mode, search, match_index] => {
            let search = decode_optional_text(search, "COPY_MODE")?;
            let match_index =
                decode_optional_positive_index(match_index, "COPY_MODE has invalid match index")?;
            if search.is_none() && match_index.is_some() {
                return Err("COPY_MODE match index requires search".to_string());
            }
            Ok(Request::CopyMode {
                session: (*session).to_string(),
                mode: decode_capture_mode(mode)?,
                search,
                match_index,
            })
        }
        ["LIST_BUFFERS"] => Ok(Request::ListBuffers { format: None }),
        ["LIST_BUFFERS_FORMAT", format] => Ok(Request::ListBuffers {
            format: Some(decode_utf8_hex(format, "LIST_BUFFERS_FORMAT")?),
        }),
        ["PASTE_BUFFER", session, buffer] => Ok(Request::PasteBuffer {
            target: Target::active((*session).to_string()),
            buffer: decode_optional_text(buffer, "PASTE_BUFFER")?,
        }),
        ["PASTE_BUFFER_TARGET", target, buffer] => Ok(Request::PasteBuffer {
            target: decode_target(target, "PASTE_BUFFER_TARGET")?,
            buffer: decode_optional_text(buffer, "PASTE_BUFFER_TARGET")?,
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
            target: Target::active((*session).to_string()),
            direction: decode_pane_resize_direction(direction)?,
            amount: amount
                .parse::<usize>()
                .ok()
                .filter(|amount| *amount > 0)
                .ok_or_else(|| "RESIZE_PANE amount must be a positive integer".to_string())?,
        }),
        ["RESIZE_PANE_TARGET", target, direction, amount] => Ok(Request::ResizePane {
            target: decode_target(target, "RESIZE_PANE_TARGET")?,
            direction: decode_pane_resize_direction(direction)?,
            amount: amount
                .parse::<usize>()
                .ok()
                .filter(|amount| *amount > 0)
                .ok_or_else(|| "RESIZE_PANE amount must be a positive integer".to_string())?,
        }),
        ["SELECT_LAYOUT", session, preset] => Ok(Request::SelectLayout {
            session: (*session).to_string(),
            window: WindowTarget::Active,
            preset: parse_layout_preset_name(preset)?,
        }),
        ["SELECT_LAYOUT_TARGET", target, preset] => {
            let target = decode_target(target, "SELECT_LAYOUT_TARGET")?;
            if target.pane != PaneTarget::Active {
                return Err("SELECT_LAYOUT target must not include a pane".to_string());
            }
            Ok(Request::SelectLayout {
                session: target.session,
                window: target.window,
                preset: parse_layout_preset_name(preset)?,
            })
        }
        ["SEND", session, hex] => Ok(Request::Send {
            target: Target::active((*session).to_string()),
            bytes: decode_hex(hex)?,
        }),
        ["SEND_TARGET", target, hex] => Ok(Request::Send {
            target: decode_target(target, "SEND_TARGET")?,
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
                target: Target::active((*session).to_string()),
                direction: decode_split_direction(direction)?,
                command,
                cwd: None,
            })
        }
        ["SPLIT_TARGET", target, direction, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "SPLIT_TARGET has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("SPLIT_TARGET argc does not match command".to_string());
            }
            Ok(Request::Split {
                target: decode_target(target, "SPLIT_TARGET")?,
                direction: decode_split_direction(direction)?,
                command,
                cwd: None,
            })
        }
        ["SPLIT_TARGET_CWD", target, direction, cwd, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "SPLIT_TARGET_CWD has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("SPLIT_TARGET_CWD argc does not match command".to_string());
            }
            Ok(Request::Split {
                target: decode_target(target, "SPLIT_TARGET_CWD")?,
                direction: decode_split_direction(direction)?,
                command,
                cwd: Some(decode_path(cwd)?),
            })
        }
        ["LIST_PANES", session] => Ok(Request::ListPanes {
            session: (*session).to_string(),
            window: WindowTarget::Active,
            format: None,
        }),
        ["LIST_PANES_FORMAT", session, format] => Ok(Request::ListPanes {
            session: (*session).to_string(),
            window: WindowTarget::Active,
            format: Some(decode_utf8_hex(format, "LIST_PANES_FORMAT")?),
        }),
        ["LIST_PANES_TARGET", session, window] => Ok(Request::ListPanes {
            session: (*session).to_string(),
            window: decode_structured_window_target(window, "LIST_PANES_TARGET")?,
            format: None,
        }),
        ["LIST_PANES_TARGET_FORMAT", session, window, format] => Ok(Request::ListPanes {
            session: (*session).to_string(),
            window: decode_structured_window_target(window, "LIST_PANES_TARGET_FORMAT")?,
            format: Some(decode_utf8_hex(format, "LIST_PANES_TARGET_FORMAT")?),
        }),
        ["SELECT_PANE", session, pane] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            window: WindowTarget::Active,
            target: PaneSelectTarget::Index(
                pane.parse::<usize>()
                    .map_err(|_| "SELECT_PANE has invalid pane index".to_string())?,
            ),
        }),
        ["SELECT_PANE_ID", session, id] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            window: WindowTarget::Active,
            target: PaneSelectTarget::Id(
                id.parse::<usize>()
                    .map_err(|_| "SELECT_PANE_ID has invalid pane id".to_string())?,
            ),
        }),
        ["SELECT_PANE_DIRECTION", session, direction] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            window: WindowTarget::Active,
            target: PaneSelectTarget::Direction(decode_pane_direction(
                direction,
                "SELECT_PANE_DIRECTION has invalid direction",
            )?),
        }),
        ["SELECT_PANE_TARGET", session, window, pane] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            window: decode_structured_window_target(window, "SELECT_PANE_TARGET")?,
            target: PaneSelectTarget::Index(
                pane.parse::<usize>()
                    .map_err(|_| "SELECT_PANE_TARGET has invalid pane index".to_string())?,
            ),
        }),
        ["SELECT_PANE_TARGET_ID", session, window, id] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            window: decode_structured_window_target(window, "SELECT_PANE_TARGET_ID")?,
            target: PaneSelectTarget::Id(
                id.parse::<usize>()
                    .map_err(|_| "SELECT_PANE_TARGET_ID has invalid pane id".to_string())?,
            ),
        }),
        ["SELECT_PANE_TARGET_DIRECTION", session, window, direction] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            window: decode_structured_window_target(window, "SELECT_PANE_TARGET_DIRECTION")?,
            target: PaneSelectTarget::Direction(decode_pane_direction(
                direction,
                "SELECT_PANE_TARGET_DIRECTION has invalid direction",
            )?),
        }),
        ["KILL_PANE", session, pane] => Ok(Request::KillPane {
            target: Target {
                session: (*session).to_string(),
                window: WindowTarget::Active,
                pane: decode_optional_pane(pane)?.map_or(PaneTarget::Active, PaneTarget::Index),
            },
        }),
        ["KILL_PANE_TARGET", target] => Ok(Request::KillPane {
            target: decode_target(target, "KILL_PANE_TARGET")?,
        }),
        ["SWAP_PANE", source, destination] => Ok(Request::SwapPane {
            source: decode_target(source, "SWAP_PANE")?,
            destination: decode_target(destination, "SWAP_PANE")?,
        }),
        ["MOVE_PANE", source, destination, direction] => Ok(Request::MovePane {
            source: decode_target(source, "MOVE_PANE")?,
            destination: decode_target(destination, "MOVE_PANE")?,
            direction: decode_split_direction(direction)?,
        }),
        ["BREAK_PANE", target] => Ok(Request::BreakPane {
            target: decode_target(target, "BREAK_PANE")?,
        }),
        ["JOIN_PANE", source, destination, direction] => Ok(Request::JoinPane {
            source: decode_target(source, "JOIN_PANE")?,
            destination: decode_target(destination, "JOIN_PANE")?,
            direction: decode_split_direction(direction)?,
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
                target: Target {
                    session: (*session).to_string(),
                    window: WindowTarget::Active,
                    pane: decode_optional_pane(pane)?.map_or(PaneTarget::Active, PaneTarget::Index),
                },
                force: match *force {
                    "0" => false,
                    "1" => true,
                    _ => return Err("RESPAWN_PANE has invalid force flag".to_string()),
                },
                command,
                cwd: None,
            })
        }
        ["RESPAWN_PANE_TARGET", target, force, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "RESPAWN_PANE_TARGET has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("RESPAWN_PANE_TARGET argc does not match command".to_string());
            }
            Ok(Request::RespawnPane {
                target: decode_target(target, "RESPAWN_PANE_TARGET")?,
                force: match *force {
                    "0" => false,
                    "1" => true,
                    _ => return Err("RESPAWN_PANE_TARGET has invalid force flag".to_string()),
                },
                command,
                cwd: None,
            })
        }
        ["RESPAWN_PANE_TARGET_CWD", target, force, cwd, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "RESPAWN_PANE_TARGET_CWD has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("RESPAWN_PANE_TARGET_CWD argc does not match command".to_string());
            }
            Ok(Request::RespawnPane {
                target: decode_target(target, "RESPAWN_PANE_TARGET_CWD")?,
                force: match *force {
                    "0" => false,
                    "1" => true,
                    _ => return Err("RESPAWN_PANE_TARGET_CWD has invalid force flag".to_string()),
                },
                command,
                cwd: Some(decode_path(cwd)?),
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
                cwd: None,
            })
        }
        ["NEW_WINDOW_CWD", session, cwd, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "NEW_WINDOW_CWD has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("NEW_WINDOW_CWD argc does not match command".to_string());
            }
            Ok(Request::NewWindow {
                session: (*session).to_string(),
                command,
                cwd: Some(decode_path(cwd)?),
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
            target: decode_optional_window(window)?
                .map_or(WindowTarget::Active, WindowTarget::Index),
        }),
        ["KILL_WINDOW_TARGET", session, target] => Ok(Request::KillWindow {
            session: (*session).to_string(),
            target: decode_structured_window_target(target, "KILL_WINDOW_TARGET")?,
        }),
        ["ZOOM_PANE", session, pane] => Ok(Request::ZoomPane {
            target: Target {
                session: (*session).to_string(),
                window: WindowTarget::Active,
                pane: decode_optional_zoom_pane(pane)?
                    .map_or(PaneTarget::Active, PaneTarget::Index),
            },
        }),
        ["ZOOM_PANE_TARGET", target] => Ok(Request::ZoomPane {
            target: decode_target(target, "ZOOM_PANE_TARGET")?,
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
        ["LIST_KEYS", format] => Ok(Request::ListKeys {
            format: decode_optional_text(format, "LIST_KEYS")?,
        }),
        ["BIND_KEY", key, command] => Ok(Request::BindKey {
            key: decode_utf8_hex(key, "BIND_KEY")?,
            command: decode_utf8_hex(command, "BIND_KEY")?,
        }),
        ["UNBIND_KEY", key] => Ok(Request::UnbindKey {
            key: decode_utf8_hex(key, "UNBIND_KEY")?,
        }),
        ["SHOW_OPTIONS", format] => Ok(Request::ShowOptions {
            format: decode_optional_text(format, "SHOW_OPTIONS")?,
        }),
        ["SET_OPTION", name, value] => Ok(Request::SetOption {
            name: decode_utf8_hex(name, "SET_OPTION")?,
            value: decode_utf8_hex(value, "SET_OPTION")?,
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

fn encode_layout_preset(preset: LayoutPreset) -> &'static str {
    match preset {
        LayoutPreset::EvenHorizontal => "even-horizontal",
        LayoutPreset::EvenVertical => "even-vertical",
        LayoutPreset::Tiled => "tiled",
        LayoutPreset::MainHorizontal => "main-horizontal",
        LayoutPreset::MainVertical => "main-vertical",
    }
}

pub fn parse_layout_preset_name(value: &str) -> Result<LayoutPreset, String> {
    match value {
        "even-horizontal" => Ok(LayoutPreset::EvenHorizontal),
        "even-vertical" => Ok(LayoutPreset::EvenVertical),
        "tiled" => Ok(LayoutPreset::Tiled),
        "main-horizontal" => Ok(LayoutPreset::MainHorizontal),
        "main-vertical" => Ok(LayoutPreset::MainVertical),
        _ => Err(format!(
            "unknown layout preset {value:?}; expected even-horizontal, even-vertical, tiled, main-horizontal, or main-vertical"
        )),
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

fn decode_optional_positive_index(
    value: &str,
    invalid_message: &str,
) -> Result<Option<usize>, String> {
    if value.is_empty() {
        Ok(None)
    } else {
        decode_positive_index(value, invalid_message).map(Some)
    }
}

fn decode_positive_index(value: &str, invalid_message: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid_message.to_string())
}

fn decode_line_offset(value: &str, invalid_message: &str) -> Result<isize, String> {
    value
        .parse::<isize>()
        .ok()
        .filter(|value| *value != 0)
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

fn encode_path(path: &Path) -> String {
    encode_hex(path.as_os_str().as_bytes())
}

fn decode_path(hex: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(OsString::from_vec(decode_hex(hex)?)))
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

    fn active_target(session: &str) -> Target {
        Target::active(session.to_string())
    }

    #[test]
    fn round_trips_new_request_with_spaced_args() {
        let command = vec!["sh".to_string(), "-c".to_string(), "echo ok".to_string()];
        let line = encode_new("dev", &command);

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::New {
                session: "dev".to_string(),
                command,
                cwd: None,
            }
        );
    }

    #[test]
    fn round_trips_new_request_with_cwd() {
        let command = vec!["pwd".to_string()];
        let cwd = PathBuf::from("/tmp/dmux cwd");
        let line = encode_new_in_cwd("dev", &command, &cwd);

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::New {
                session: "dev".to_string(),
                command,
                cwd: Some(cwd),
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
                target: active_target("dev"),
                direction: PaneResizeDirection::Left,
                amount: 5,
            }
        );
    }

    #[test]
    fn round_trips_select_layout_request() {
        let target = Target {
            session: "dev".to_string(),
            window: WindowTarget::Id(7),
            pane: PaneTarget::Active,
        };
        let line = encode_select_layout_target(&target, LayoutPreset::Tiled);

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SelectLayout {
                session: "dev".to_string(),
                window: WindowTarget::Id(7),
                preset: LayoutPreset::Tiled,
            }
        );
    }

    #[test]
    fn round_trips_pane_movement_requests() {
        let source = Target {
            session: "dev".to_string(),
            window: WindowTarget::Index(0),
            pane: PaneTarget::Id(10),
        };
        let destination = Target {
            session: "dev".to_string(),
            window: WindowTarget::Id(7),
            pane: PaneTarget::Index(1),
        };

        assert_eq!(
            decode_request(&encode_swap_pane(&source, &destination)).unwrap(),
            Request::SwapPane {
                source: source.clone(),
                destination: destination.clone(),
            }
        );
        assert_eq!(
            decode_request(&encode_move_pane(
                &source,
                &destination,
                SplitDirection::Vertical
            ))
            .unwrap(),
            Request::MovePane {
                source: source.clone(),
                destination: destination.clone(),
                direction: SplitDirection::Vertical,
            }
        );
        assert_eq!(
            decode_request(&encode_break_pane(&source)).unwrap(),
            Request::BreakPane {
                target: source.clone(),
            }
        );
        assert_eq!(
            decode_request(&encode_join_pane(
                &source,
                &destination,
                SplitDirection::Horizontal
            ))
            .unwrap(),
            Request::JoinPane {
                source,
                destination,
                direction: SplitDirection::Horizontal,
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
                target: active_target("dev"),
                mode: CaptureMode::Screen,
                selection: BufferSelection::All,
            }
        );
    }

    #[test]
    fn round_trips_capture_history_request() {
        let line = encode_capture("dev", CaptureMode::History);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Capture {
                target: active_target("dev"),
                mode: CaptureMode::History,
                selection: BufferSelection::All,
            }
        );
    }

    #[test]
    fn decodes_legacy_capture_request_as_all() {
        assert_eq!(
            decode_request("CAPTURE\tdev\n").unwrap(),
            Request::Capture {
                target: active_target("dev"),
                mode: CaptureMode::All,
                selection: BufferSelection::All,
            }
        );
    }

    #[test]
    fn round_trips_capture_line_range_request() {
        let line = encode_capture_with_selection(
            "dev",
            CaptureMode::Screen,
            BufferSelection::LineRange { start: -2, end: -1 },
        );
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Capture {
                target: active_target("dev"),
                mode: CaptureMode::Screen,
                selection: BufferSelection::LineRange { start: -2, end: -1 },
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
            BufferSelection::Search {
                needle: "needle".to_string(),
                match_index: 2,
            },
        );
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SaveBuffer {
                target: active_target("dev"),
                buffer: Some("match".to_string()),
                mode: CaptureMode::All,
                selection: BufferSelection::Search {
                    needle: "needle".to_string(),
                    match_index: 2,
                },
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
        let line = encode_copy_mode("dev", CaptureMode::History, Some("needle"), Some(2));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::CopyMode {
                session: "dev".to_string(),
                mode: CaptureMode::History,
                search: Some("needle".to_string()),
                match_index: Some(2),
            }
        );
    }

    #[test]
    fn round_trips_list_buffers_format_request() {
        let line = encode_list_buffers(Some("#{buffer.name}"));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ListBuffers {
                format: Some("#{buffer.name}".to_string()),
            }
        );
    }

    #[test]
    fn round_trips_paste_buffer_request() {
        let line = encode_paste_buffer("dev", Some("saved"));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::PasteBuffer {
                target: active_target("dev"),
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
                target: active_target("dev"),
                bytes: b"hello\r".to_vec(),
            }
        );
    }

    #[test]
    fn round_trips_structured_send_target() {
        let target = Target {
            session: "dev".to_string(),
            window: WindowTarget::Id(7),
            pane: PaneTarget::Id(42),
        };
        let line = encode_send_target(&target, b"hello");

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Send {
                target,
                bytes: b"hello".to_vec(),
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
                target: Target::active("dev".to_string()),
                direction: SplitDirection::Horizontal,
                command,
                cwd: None,
            }
        );
    }

    #[test]
    fn round_trips_targeted_split_request() {
        let command = vec!["sh".to_string(), "-c".to_string(), "echo split".to_string()];
        let target = Target {
            session: "dev".to_string(),
            window: WindowTarget::Index(1),
            pane: PaneTarget::Id(42),
        };
        let line = encode_split_target(&target, SplitDirection::Vertical, &command);

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Split {
                target,
                direction: SplitDirection::Vertical,
                command,
                cwd: None,
            }
        );
    }

    #[test]
    fn round_trips_targeted_split_request_with_cwd() {
        let command = vec!["pwd".to_string()];
        let cwd = PathBuf::from("/tmp/dmux split cwd");
        let target = Target {
            session: "dev".to_string(),
            window: WindowTarget::Active,
            pane: PaneTarget::Index(0),
        };
        let line = encode_split_target_in_cwd(&target, SplitDirection::Horizontal, &command, &cwd);

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Split {
                target,
                direction: SplitDirection::Horizontal,
                command,
                cwd: Some(cwd),
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
                window: WindowTarget::Active,
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
                window: WindowTarget::Active,
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
                window: WindowTarget::Active,
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
                target: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Active,
                    pane: PaneTarget::Index(1),
                },
            }
        );
    }

    #[test]
    fn round_trips_kill_pane_request_for_active_pane() {
        let line = encode_kill_pane("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::KillPane {
                target: active_target("dev"),
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
                target: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Active,
                    pane: PaneTarget::Index(1),
                },
                force: true,
                command,
                cwd: None,
            }
        );
    }

    #[test]
    fn round_trips_respawn_pane_request_with_cwd() {
        let command = vec!["pwd".to_string()];
        let cwd = PathBuf::from("/tmp/dmux respawn cwd");
        let target = Target {
            session: "dev".to_string(),
            window: WindowTarget::Active,
            pane: PaneTarget::Index(1),
        };
        let line = encode_respawn_pane_target_in_cwd(&target, true, &command, &cwd);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::RespawnPane {
                target,
                force: true,
                command,
                cwd: Some(cwd),
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
                cwd: None,
            }
        );
    }

    #[test]
    fn round_trips_new_window_request_with_cwd() {
        let command = vec!["pwd".to_string()];
        let cwd = PathBuf::from("/tmp/dmux window cwd");
        let line = encode_new_window_in_cwd("dev", &command, &cwd);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::NewWindow {
                session: "dev".to_string(),
                command,
                cwd: Some(cwd),
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
                target: WindowTarget::Index(1),
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
                target: WindowTarget::Active,
            }
        );
    }

    #[test]
    fn round_trips_zoom_pane_request_for_active_pane() {
        let line = encode_zoom_pane("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ZoomPane {
                target: active_target("dev"),
            }
        );
    }

    #[test]
    fn round_trips_zoom_pane_request_with_index() {
        let line = encode_zoom_pane("dev", Some(0));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ZoomPane {
                target: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Active,
                    pane: PaneTarget::Index(0),
                },
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
                window: WindowTarget::Active,
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

    #[test]
    fn round_trips_key_and_option_requests() {
        assert_eq!(
            decode_request(&encode_list_keys(Some("#{key}\t#{command}"))).unwrap(),
            Request::ListKeys {
                format: Some("#{key}\t#{command}".to_string()),
            }
        );
        assert_eq!(
            decode_request(&encode_bind_key("C-a", "copy-mode")).unwrap(),
            Request::BindKey {
                key: "C-a".to_string(),
                command: "copy-mode".to_string(),
            }
        );
        assert_eq!(
            decode_request(&encode_unbind_key("x")).unwrap(),
            Request::UnbindKey {
                key: "x".to_string(),
            }
        );
        assert_eq!(
            decode_request(&encode_show_options(Some("#{option.name}=#{option.value}"))).unwrap(),
            Request::ShowOptions {
                format: Some("#{option.name}=#{option.value}".to_string()),
            }
        );
        assert_eq!(
            decode_request(&encode_set_option("prefix", "C-a")).unwrap(),
            Request::SetOption {
                name: "prefix".to_string(),
                value: "C-a".to_string(),
            }
        );
    }
}
