use crate::protocol::{self, BufferSelection, CaptureMode, Request, SplitDirection};
use crate::pty::{self, PtySize, SpawnSpec};
use crate::term::TerminalState;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const MAX_HISTORY_BYTES: usize = 1024 * 1024;
const MAX_BUFFER_BYTES: usize = 1024 * 1024;
const MAX_BUFFERS: usize = 50;

pub fn run(socket_path: PathBuf) -> io::Result<()> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    let state = Arc::new(ServerState {
        sessions: Mutex::new(HashMap::new()),
        buffers: Mutex::new(BufferStore::new()),
        socket_path,
    });

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    let _ = handle_connection(state, stream);
                });
            }
            Err(err) => return Err(err),
        }
    }

    Ok(())
}

struct ServerState {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    buffers: Mutex<BufferStore>,
    socket_path: PathBuf,
}

struct Buffer {
    name: String,
    text: String,
}

struct BufferDescription {
    name: String,
    bytes: usize,
    preview: String,
}

struct BufferStore {
    buffers: Vec<Buffer>,
    next_auto: usize,
}

impl BufferStore {
    fn new() -> Self {
        Self {
            buffers: Vec::new(),
            next_auto: 0,
        }
    }

    fn save(&mut self, name: Option<&str>, text: String) -> Result<String, String> {
        if text.len() > MAX_BUFFER_BYTES {
            return Err("buffer exceeds maximum size".to_string());
        }

        let name = match name {
            Some(name) if name.is_empty() => return Err("buffer name cannot be empty".to_string()),
            Some(name) => name.to_string(),
            None => self.next_name(),
        };

        if let Some(index) = self.buffers.iter().position(|buffer| buffer.name == name) {
            self.buffers.remove(index);
        }

        self.buffers.push(Buffer {
            name: name.clone(),
            text,
        });
        while self.buffers.len() > MAX_BUFFERS {
            self.buffers.remove(0);
        }
        Ok(name)
    }

    fn list(&self) -> Vec<BufferDescription> {
        self.buffers
            .iter()
            .map(|buffer| BufferDescription {
                name: buffer.name.clone(),
                bytes: buffer.text.len(),
                preview: buffer_preview(&buffer.text),
            })
            .collect()
    }

    fn resolve(&self, name: Option<&str>) -> Option<&Buffer> {
        match name {
            Some(name) => self.buffers.iter().find(|buffer| buffer.name == name),
            None => self.buffers.last(),
        }
    }

    fn delete(&mut self, name: &str) -> bool {
        let Some(index) = self.buffers.iter().position(|buffer| buffer.name == name) else {
            return false;
        };
        self.buffers.remove(index);
        true
    }

    fn next_name(&mut self) -> String {
        loop {
            let name = format!("buffer-{}", self.next_auto);
            self.next_auto += 1;
            if !self.buffers.iter().any(|buffer| buffer.name == name) {
                return name;
            }
        }
    }
}

fn buffer_preview(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .replace('\t', " ")
        .chars()
        .take(40)
        .collect()
}

fn format_copy_mode_lines(text: &str, search: Option<&str>) -> String {
    let mut output = String::new();
    for (index, line) in text.lines().enumerate() {
        if search.is_none_or(|needle| line.contains(needle)) {
            output.push_str(&(index + 1).to_string());
            output.push('\t');
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

fn select_buffer_text(text: &str, selection: &BufferSelection) -> Result<String, String> {
    match selection {
        BufferSelection::All => Ok(text.to_string()),
        BufferSelection::LineRange { start, end } => select_line_range(text, *start, *end),
        BufferSelection::Search(needle) => select_search_match(text, needle),
    }
}

fn select_line_range(text: &str, start: usize, end: usize) -> Result<String, String> {
    if start == 0 || end == 0 || start > end {
        return Err("invalid line range".to_string());
    }

    let lines = text.lines().collect::<Vec<_>>();
    if end > lines.len() {
        return Err("missing line".to_string());
    }

    Ok(join_selected_lines(&lines[start - 1..end]))
}

fn select_search_match(text: &str, needle: &str) -> Result<String, String> {
    if needle.is_empty() {
        return Err("search text cannot be empty".to_string());
    }

    let Some(line) = text.lines().find(|line| line.contains(needle)) else {
        return Err("missing match".to_string());
    };

    Ok(join_selected_lines(&[line]))
}

fn join_selected_lines(lines: &[&str]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        let mut text = lines.join("\n");
        text.push('\n');
        text
    }
}

struct Session {
    windows: Mutex<WindowSet>,
}

impl Session {
    fn new(pane: Arc<Pane>) -> Self {
        Self {
            windows: Mutex::new(WindowSet::new(Window::new(pane))),
        }
    }

    fn active_pane(&self) -> Option<Arc<Pane>> {
        self.windows.lock().unwrap().active_pane()
    }

    fn add_pane(&self, pane: Arc<Pane>) {
        self.windows.lock().unwrap().add_pane(pane);
    }

    fn select_pane(&self, index: usize) -> bool {
        self.windows.lock().unwrap().select_pane(index)
    }

    fn kill_pane(&self, target: Option<usize>) -> Result<(), String> {
        self.windows.lock().unwrap().kill_pane(target)
    }

    fn pane_descriptions(&self) -> Vec<PaneDescription> {
        self.windows.lock().unwrap().pane_descriptions()
    }

    fn panes(&self) -> Vec<Arc<Pane>> {
        self.windows.lock().unwrap().panes()
    }

    fn add_window(&self, pane: Arc<Pane>) {
        self.windows.lock().unwrap().add(Window::new(pane));
    }

    fn select_window(&self, index: usize) -> bool {
        self.windows.lock().unwrap().select(index)
    }

    fn window_count(&self) -> usize {
        self.windows.lock().unwrap().len()
    }

    fn kill_window(&self, target: Option<usize>) -> Result<(), String> {
        self.windows.lock().unwrap().kill_window(target)
    }

    fn zoom_pane(&self, target: Option<usize>) -> Result<(), String> {
        self.windows.lock().unwrap().zoom_pane(target)
    }

    fn status_context(&self, name: &str) -> Option<StatusContext> {
        self.windows.lock().unwrap().status_context(name)
    }

    fn attach_panes(&self) -> Vec<IndexedPane> {
        self.windows.lock().unwrap().attach_panes()
    }
}

struct Window {
    panes: PaneSet<Arc<Pane>>,
    zoomed: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LayoutNode {
    Pane(usize),
    Split {
        direction: SplitDirection,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    fn split_pane(
        &mut self,
        target: usize,
        direction: SplitDirection,
        new_index: usize,
    ) -> bool {
        match self {
            LayoutNode::Pane(index) if *index == target => {
                *self = LayoutNode::Split {
                    direction,
                    first: Box::new(LayoutNode::Pane(target)),
                    second: Box::new(LayoutNode::Pane(new_index)),
                };
                true
            }
            LayoutNode::Pane(_) => false,
            LayoutNode::Split { first, second, .. } => {
                first.split_pane(target, direction, new_index)
                    || second.split_pane(target, direction, new_index)
            }
        }
    }

    fn remove_pane(&mut self, removed: usize) -> bool {
        match self {
            LayoutNode::Pane(index) if *index == removed => false,
            LayoutNode::Pane(index) => {
                if *index > removed {
                    *index -= 1;
                }
                true
            }
            LayoutNode::Split { first, second, .. } => {
                let keep_first = first.remove_pane(removed);
                let keep_second = second.remove_pane(removed);
                match (keep_first, keep_second) {
                    (true, true) => true,
                    (true, false) => {
                        *self = (**first).clone();
                        true
                    }
                    (false, true) => {
                        *self = (**second).clone();
                        true
                    }
                    (false, false) => false,
                }
            }
        }
    }
}

impl Window {
    fn new(pane: Arc<Pane>) -> Self {
        Self {
            panes: PaneSet::new(pane),
            zoomed: None,
        }
    }

    fn active_pane(&self) -> Option<Arc<Pane>> {
        self.panes.active()
    }

    fn add_pane(&mut self, pane: Arc<Pane>) {
        self.panes.add(pane);
        if self.zoomed.is_some() {
            self.zoomed = Some(self.panes.active_index());
        }
    }

    fn select_pane(&mut self, index: usize) -> bool {
        let selected = self.panes.select(index);
        if selected && self.zoomed.is_some() {
            self.zoomed = Some(index);
        }
        selected
    }

    fn kill_pane(&mut self, target: Option<usize>) -> Result<(), String> {
        let index = self.panes.kill_index(target).map_err(str::to_string)?;
        let pane = self
            .panes
            .get(index)
            .expect("validated pane index must exist while session lock is held");
        pty::terminate(pane.child_pid).map_err(|err| err.to_string())?;
        self.panes.kill_at(index);
        self.adjust_zoom_after_pane_removal(index);
        Ok(())
    }

    fn panes(&self) -> Vec<Arc<Pane>> {
        self.panes.all()
    }

    fn pane_descriptions(&self) -> Vec<PaneDescription> {
        let window_zoomed = self.zoomed.is_some();
        (0..self.panes.len())
            .map(|index| PaneDescription {
                index,
                active: index == self.panes.active_index(),
                zoomed: self.zoomed == Some(index),
                window_zoomed,
            })
            .collect()
    }

    fn attach_panes(&self) -> Vec<IndexedPane> {
        if let Some(index) = self.zoomed {
            return self
                .panes
                .get(index)
                .cloned()
                .map(|pane| vec![IndexedPane { index, pane }])
                .unwrap_or_default();
        }

        (0..self.panes.len())
            .filter_map(|index| {
                self.panes
                    .get(index)
                    .cloned()
                    .map(|pane| IndexedPane { index, pane })
            })
            .collect()
    }

    fn active_pane_index(&self) -> usize {
        self.panes.active_index()
    }

    fn active_pane_zoomed(&self) -> bool {
        self.zoomed == Some(self.panes.active_index())
    }

    fn is_zoomed(&self) -> bool {
        self.zoomed.is_some()
    }

    fn zoom_pane(&mut self, target: Option<usize>) -> Result<(), String> {
        let index = target.unwrap_or(self.panes.active_index());
        if index >= self.panes.len() {
            return Err("missing pane".to_string());
        }

        if self.zoomed == Some(index) {
            self.zoomed = None;
            return Ok(());
        }

        self.panes.select(index);
        self.zoomed = Some(index);
        Ok(())
    }

    fn adjust_zoom_after_pane_removal(&mut self, removed: usize) {
        match self.zoomed {
            Some(zoomed) if zoomed == removed => self.zoomed = None,
            Some(zoomed) if zoomed > removed => self.zoomed = Some(zoomed - 1),
            _ => {}
        }
    }

    fn terminate_panes(&self) -> Result<(), String> {
        for pane in self.panes() {
            pty::terminate(pane.child_pid).map_err(|err| err.to_string())?;
        }
        Ok(())
    }
}

struct WindowSet {
    windows: Vec<Window>,
    active: usize,
}

impl WindowSet {
    fn new(window: Window) -> Self {
        Self {
            windows: vec![window],
            active: 0,
        }
    }

    fn add(&mut self, window: Window) {
        self.windows.push(window);
        self.active = self.windows.len() - 1;
    }

    fn select(&mut self, index: usize) -> bool {
        if index >= self.windows.len() {
            return false;
        }
        self.active = index;
        true
    }

    fn active_window(&self) -> Option<&Window> {
        self.windows.get(self.active)
    }

    fn active_window_mut(&mut self) -> Option<&mut Window> {
        self.windows.get_mut(self.active)
    }

    fn active_pane(&self) -> Option<Arc<Pane>> {
        self.active_window()?.active_pane()
    }

    fn add_pane(&mut self, pane: Arc<Pane>) {
        if let Some(window) = self.active_window_mut() {
            window.add_pane(pane);
        }
    }

    fn select_pane(&mut self, index: usize) -> bool {
        self.active_window_mut()
            .is_some_and(|window| window.select_pane(index))
    }

    fn kill_pane(&mut self, target: Option<usize>) -> Result<(), String> {
        self.active_window_mut()
            .ok_or_else(|| "missing window".to_string())?
            .kill_pane(target)
    }

    fn panes(&self) -> Vec<Arc<Pane>> {
        self.windows
            .iter()
            .flat_map(Window::panes)
            .collect::<Vec<_>>()
    }

    fn pane_descriptions(&self) -> Vec<PaneDescription> {
        self.active_window()
            .map_or_else(Vec::new, Window::pane_descriptions)
    }

    fn attach_panes(&self) -> Vec<IndexedPane> {
        self.active_window()
            .map_or_else(Vec::new, Window::attach_panes)
    }

    fn status_context(&self, session_name: &str) -> Option<StatusContext> {
        let window = self.active_window()?;
        Some(StatusContext {
            session_name: session_name.to_string(),
            window_index: self.active,
            window_count: self.windows.len(),
            pane_index: window.active_pane_index(),
            pane_zoomed: window.active_pane_zoomed(),
            window_zoomed: window.is_zoomed(),
        })
    }

    fn len(&self) -> usize {
        self.windows.len()
    }

    fn kill_window(&mut self, target: Option<usize>) -> Result<(), String> {
        if self.windows.len() <= 1 {
            return Err("cannot kill last window; use kill-session".to_string());
        }

        let index = target.unwrap_or(self.active);
        if index >= self.windows.len() {
            return Err("missing window".to_string());
        }

        self.windows[index].terminate_panes()?;
        self.windows.remove(index);
        if self.active == index {
            self.active = index.saturating_sub(1).min(self.windows.len() - 1);
        } else if self.active > index {
            self.active -= 1;
        }
        Ok(())
    }

    fn zoom_pane(&mut self, target: Option<usize>) -> Result<(), String> {
        self.active_window_mut()
            .ok_or_else(|| "missing window".to_string())?
            .zoom_pane(target)
    }
}

struct PaneDescription {
    index: usize,
    active: bool,
    zoomed: bool,
    window_zoomed: bool,
}

struct IndexedPane {
    index: usize,
    pane: Arc<Pane>,
}

struct PaneSnapshot {
    index: usize,
    screen: String,
}

struct StatusContext {
    session_name: String,
    window_index: usize,
    window_count: usize,
    pane_index: usize,
    pane_zoomed: bool,
    window_zoomed: bool,
}

struct PaneSet<T> {
    panes: Vec<T>,
    active: usize,
}

impl<T> PaneSet<T> {
    fn new(pane: T) -> Self {
        Self {
            panes: vec![pane],
            active: 0,
        }
    }

    fn add(&mut self, pane: T) {
        self.panes.push(pane);
        self.active = self.panes.len() - 1;
    }

    fn select(&mut self, index: usize) -> bool {
        if index >= self.panes.len() {
            return false;
        }
        self.active = index;
        true
    }

    #[cfg(test)]
    fn kill(&mut self, target: Option<usize>) -> Result<T, &'static str> {
        let index = self.kill_index(target)?;
        Ok(self.kill_at(index))
    }

    fn kill_index(&self, target: Option<usize>) -> Result<usize, &'static str> {
        if self.panes.len() <= 1 {
            return Err("cannot kill last pane; use kill-session");
        }

        let index = target.unwrap_or(self.active);
        if index >= self.panes.len() {
            return Err("missing pane");
        }

        Ok(index)
    }

    fn kill_at(&mut self, index: usize) -> T {
        let pane = self.panes.remove(index);
        if self.active == index {
            self.active = index.saturating_sub(1).min(self.panes.len() - 1);
        } else if self.active > index {
            self.active -= 1;
        }
        pane
    }

    fn get(&self, index: usize) -> Option<&T> {
        self.panes.get(index)
    }

    fn active_index(&self) -> usize {
        self.active
    }

    fn len(&self) -> usize {
        self.panes.len()
    }
}

impl<T: Clone> PaneSet<T> {
    fn active(&self) -> Option<T> {
        self.panes.get(self.active).cloned()
    }

    fn all(&self) -> Vec<T> {
        self.panes.clone()
    }
}

struct Pane {
    child_pid: i32,
    writer: Arc<Mutex<File>>,
    size: Mutex<PtySize>,
    raw_history: Arc<Mutex<Vec<u8>>>,
    terminal: Arc<Mutex<TerminalState>>,
    clients: Arc<Mutex<Vec<UnixStream>>>,
}

fn handle_connection(state: Arc<ServerState>, mut stream: UnixStream) -> io::Result<()> {
    let mut line = String::new();
    {
        let mut reader = io::BufReader::new(stream.try_clone()?);
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
    }

    let request = match protocol::decode_request(&line) {
        Ok(request) => request,
        Err(err) => {
            write_err(&mut stream, &err)?;
            return Ok(());
        }
    };

    match request {
        Request::New { session, command } => handle_new(&state, &mut stream, session, command),
        Request::List => handle_list(&state, &mut stream),
        Request::Capture { session, mode } => handle_capture(&state, &mut stream, &session, mode),
        Request::SaveBuffer {
            session,
            buffer,
            mode,
            selection,
        } => handle_save_buffer(
            &state,
            &mut stream,
            &session,
            buffer.as_deref(),
            mode,
            &selection,
        ),
        Request::CopyMode {
            session,
            mode,
            search,
        } => handle_copy_mode(&state, &mut stream, &session, mode, search.as_deref()),
        Request::ListBuffers => handle_list_buffers(&state, &mut stream),
        Request::PasteBuffer { session, buffer } => {
            handle_paste_buffer(&state, &mut stream, &session, buffer.as_deref())
        }
        Request::DeleteBuffer { buffer } => handle_delete_buffer(&state, &mut stream, &buffer),
        Request::Resize {
            session,
            cols,
            rows,
        } => handle_resize(&state, &mut stream, &session, cols, rows),
        Request::Send { session, bytes } => handle_send(&state, &mut stream, &session, &bytes),
        Request::Split {
            session,
            direction: _,
            command,
        } => handle_split(&state, &mut stream, &session, command),
        Request::ListPanes { session, format } => {
            handle_list_panes(&state, &mut stream, &session, format.as_deref())
        }
        Request::SelectPane { session, pane } => {
            handle_select_pane(&state, &mut stream, &session, pane)
        }
        Request::KillPane { session, pane } => {
            handle_kill_pane(&state, &mut stream, &session, pane)
        }
        Request::NewWindow { session, command } => {
            handle_new_window(&state, &mut stream, &session, command)
        }
        Request::ListWindows { session } => handle_list_windows(&state, &mut stream, &session),
        Request::SelectWindow { session, window } => {
            handle_select_window(&state, &mut stream, &session, window)
        }
        Request::KillWindow { session, window } => {
            handle_kill_window(&state, &mut stream, &session, window)
        }
        Request::ZoomPane { session, pane } => {
            handle_zoom_pane(&state, &mut stream, &session, pane)
        }
        Request::StatusLine { session, format } => {
            handle_status_line(&state, &mut stream, &session, format.as_deref())
        }
        Request::DisplayMessage { session, format } => {
            handle_display_message(&state, &mut stream, &session, &format)
        }
        Request::Kill { session } => handle_kill(&state, &mut stream, &session),
        Request::KillServer => handle_kill_server(&state, &mut stream),
        Request::Attach { session } => handle_attach(&state, stream, &session),
        Request::AttachSnapshot { session } => {
            handle_attach_snapshot(&state, &mut stream, &session)
        }
    }
}

fn handle_new(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: String,
    command: Vec<String>,
) -> io::Result<()> {
    let mut sessions = state.sessions.lock().unwrap();
    if sessions.contains_key(&name) {
        write_err(stream, "session already exists")?;
        return Ok(());
    }

    let cwd = std::env::current_dir()?;
    let pane = spawn_pane(name.clone(), command, cwd, PtySize { cols: 80, rows: 24 })?;
    let session = Arc::new(Session::new(pane));
    sessions.insert(name, session);
    write_ok(stream)
}

fn spawn_pane(
    session_name: String,
    command: Vec<String>,
    cwd: PathBuf,
    size: PtySize,
) -> io::Result<Arc<Pane>> {
    let mut spec = SpawnSpec::new(session_name, command, cwd);
    spec.size = size;
    let process = pty::spawn(&spec)?;
    let reader = process.master.try_clone()?;
    let pane = Arc::new(Pane {
        child_pid: process.child_pid,
        writer: Arc::new(Mutex::new(process.master)),
        size: Mutex::new(spec.size),
        raw_history: Arc::new(Mutex::new(Vec::new())),
        terminal: Arc::new(Mutex::new(TerminalState::new(
            spec.size.cols as usize,
            spec.size.rows as usize,
            10_000,
        ))),
        clients: Arc::new(Mutex::new(Vec::new())),
    });

    start_output_pump(reader, Arc::clone(&pane));
    Ok(pane)
}

fn handle_list(state: &Arc<ServerState>, stream: &mut UnixStream) -> io::Result<()> {
    let mut names = state
        .sessions
        .lock()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort();

    write_ok(stream)?;
    for name in names {
        writeln!(stream, "{name}")?;
    }
    Ok(())
}

fn handle_capture(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    mode: CaptureMode,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(pane) = session.active_pane() else {
        write_err(stream, "missing pane")?;
        return Ok(());
    };

    write_ok(stream)?;
    let captured = capture_pane_text(&pane, mode);
    stream.write_all(captured.as_bytes())
}

fn capture_pane_text(pane: &Pane, mode: CaptureMode) -> String {
    let terminal = pane.terminal.lock().unwrap();
    match mode {
        CaptureMode::Screen => terminal.capture_screen_text(),
        CaptureMode::History => terminal.capture_history_text(),
        CaptureMode::All => terminal.capture_text(),
    }
}

fn handle_save_buffer(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    buffer: Option<&str>,
    mode: CaptureMode,
    selection: &BufferSelection,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(pane) = session.active_pane() else {
        write_err(stream, "missing pane")?;
        return Ok(());
    };

    let captured = capture_pane_text(&pane, mode);
    let selected = match select_buffer_text(&captured, selection) {
        Ok(text) => text,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };
    let saved_name = match state.buffers.lock().unwrap().save(buffer, selected) {
        Ok(name) => name,
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };

    write_ok(stream)?;
    writeln!(stream, "{saved_name}")
}

fn handle_list_buffers(state: &Arc<ServerState>, stream: &mut UnixStream) -> io::Result<()> {
    let buffers = state.buffers.lock().unwrap().list();

    write_ok(stream)?;
    for buffer in buffers {
        writeln!(
            stream,
            "{}\t{}\t{}",
            buffer.name, buffer.bytes, buffer.preview
        )?;
    }
    Ok(())
}

fn handle_copy_mode(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    mode: CaptureMode,
    search: Option<&str>,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(pane) = session.active_pane() else {
        write_err(stream, "missing pane")?;
        return Ok(());
    };

    let captured = capture_pane_text(&pane, mode);
    let output = format_copy_mode_lines(&captured, search);
    write_ok(stream)?;
    stream.write_all(output.as_bytes())
}

fn handle_paste_buffer(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    buffer: Option<&str>,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(pane) = session.active_pane() else {
        write_err(stream, "missing pane")?;
        return Ok(());
    };
    let Some(text) = state
        .buffers
        .lock()
        .unwrap()
        .resolve(buffer)
        .map(|buffer| buffer.text.clone())
    else {
        write_err(stream, "missing buffer")?;
        return Ok(());
    };

    pane.writer.lock().unwrap().write_all(text.as_bytes())?;
    write_ok(stream)
}

fn handle_delete_buffer(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    buffer: &str,
) -> io::Result<()> {
    if !state.buffers.lock().unwrap().delete(buffer) {
        write_err(stream, "missing buffer")?;
        return Ok(());
    }

    write_ok(stream)
}

fn handle_resize(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let size = match PtySize::new(cols, rows) {
        Ok(size) => size,
        Err(err) => {
            write_err(stream, &err.to_string())?;
            return Ok(());
        }
    };
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(pane) = session.active_pane() else {
        write_err(stream, "missing pane")?;
        return Ok(());
    };

    {
        let writer = pane.writer.lock().unwrap();
        pty::resize(&writer, size)?;
    }
    *pane.size.lock().unwrap() = size;
    pane.terminal
        .lock()
        .unwrap()
        .resize(size.cols as usize, size.rows as usize);

    write_ok(stream)
}

fn handle_send(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    bytes: &[u8],
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(pane) = session.active_pane() else {
        write_err(stream, "missing pane")?;
        return Ok(());
    };

    pane.writer.lock().unwrap().write_all(bytes)?;
    write_ok(stream)
}

fn handle_split(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    command: Vec<String>,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(active) = session.active_pane() else {
        write_err(stream, "missing pane")?;
        return Ok(());
    };

    let size = *active.size.lock().unwrap();
    let cwd = std::env::current_dir()?;
    let pane = spawn_pane(name.to_string(), command, cwd, size)?;
    session.add_pane(pane);
    write_ok(stream)
}

fn handle_list_panes(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    _format: Option<&str>,
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
    for pane in session.pane_descriptions() {
        match _format {
            Some(format) => writeln!(stream, "{}", format_pane_line(format, &pane))?,
            None => writeln!(stream, "{}", pane.index)?,
        }
    }
    Ok(())
}

fn format_pane_line(format: &str, pane: &PaneDescription) -> String {
    format
        .replace("#{pane.index}", &pane.index.to_string())
        .replace("#{pane.active}", if pane.active { "1" } else { "0" })
        .replace("#{pane.zoomed}", if pane.zoomed { "1" } else { "0" })
        .replace(
            "#{window.zoomed_flag}",
            if pane.window_zoomed { "1" } else { "0" },
        )
}

fn handle_zoom_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    pane: Option<usize>,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    if let Err(message) = session.zoom_pane(pane) {
        write_err(stream, &message)?;
        return Ok(());
    }

    write_ok(stream)
}

fn handle_status_line(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    format: Option<&str>,
) -> io::Result<()> {
    let context = status_context(state, name);
    let Some(context) = context else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    let format = format.unwrap_or("#{session.name} #{window.list} pane #{pane.index}");
    writeln!(stream, "{}", format_status_line(format, &context))
}

fn handle_display_message(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    format: &str,
) -> io::Result<()> {
    let context = status_context(state, name);
    let Some(context) = context else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    write_ok(stream)?;
    writeln!(stream, "{}", format_status_line(format, &context))
}

fn status_context(state: &Arc<ServerState>, name: &str) -> Option<StatusContext> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    }?;

    session.status_context(name)
}

fn format_status_line(format: &str, context: &StatusContext) -> String {
    let window_index = context.window_index.to_string();
    let window_list = format_window_list(context);
    let pane_index = context.pane_index.to_string();
    let pane_zoomed = if context.pane_zoomed { "1" } else { "0" };
    let window_zoomed = if context.window_zoomed { "1" } else { "0" };
    let replacements = [
        ("#{session.name}", context.session_name.as_str()),
        ("#{window.index}", window_index.as_str()),
        ("#{window.list}", window_list.as_str()),
        ("#{pane.index}", pane_index.as_str()),
        ("#{pane.zoomed}", pane_zoomed),
        ("#{window.zoomed_flag}", window_zoomed),
    ];
    let mut output = String::with_capacity(format.len());
    let mut remaining = format;

    while !remaining.is_empty() {
        if let Some((token, value)) = replacements
            .iter()
            .find(|(token, _)| remaining.starts_with(*token))
        {
            output.push_str(value);
            remaining = &remaining[token.len()..];
        } else {
            let ch = remaining
                .chars()
                .next()
                .expect("non-empty string must contain a character");
            output.push(ch);
            remaining = &remaining[ch.len_utf8()..];
        }
    }

    output
}

fn format_window_list(context: &StatusContext) -> String {
    (0..context.window_count)
        .map(|index| {
            if index == context.window_index {
                format!("[{index}]")
            } else {
                index.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn handle_select_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    index: usize,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    if !session.select_pane(index) {
        write_err(stream, "missing pane")?;
        return Ok(());
    }

    write_ok(stream)
}

fn handle_kill_pane(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    pane: Option<usize>,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    match session.kill_pane(pane) {
        Ok(()) => {}
        Err(message) => {
            write_err(stream, &message)?;
            return Ok(());
        }
    };

    write_ok(stream)
}

fn handle_new_window(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    command: Vec<String>,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };
    let Some(active) = session.active_pane() else {
        write_err(stream, "missing pane")?;
        return Ok(());
    };

    let size = *active.size.lock().unwrap();
    let cwd = std::env::current_dir()?;
    let pane = spawn_pane(name.to_string(), command, cwd, size)?;
    session.add_window(pane);
    write_ok(stream)
}

fn handle_list_windows(
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
    for index in 0..session.window_count() {
        writeln!(stream, "{index}")?;
    }
    Ok(())
}

fn handle_select_window(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    index: usize,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    if !session.select_window(index) {
        write_err(stream, "missing window")?;
        return Ok(());
    }

    write_ok(stream)
}

fn handle_kill_window(
    state: &Arc<ServerState>,
    stream: &mut UnixStream,
    name: &str,
    window: Option<usize>,
) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    if let Err(message) = session.kill_window(window) {
        write_err(stream, &message)?;
        return Ok(());
    }

    write_ok(stream)
}

fn handle_kill(state: &Arc<ServerState>, stream: &mut UnixStream, name: &str) -> io::Result<()> {
    let session = state.sessions.lock().unwrap().remove(name);
    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    for pane in session.panes() {
        let _ = pty::terminate(pane.child_pid);
    }
    write_ok(stream)
}

fn handle_kill_server(state: &Arc<ServerState>, stream: &mut UnixStream) -> io::Result<()> {
    let sessions = state
        .sessions
        .lock()
        .unwrap()
        .drain()
        .map(|(_, session)| session)
        .collect::<Vec<_>>();
    for session in sessions {
        for pane in session.panes() {
            let _ = pty::terminate(pane.child_pid);
        }
    }

    write_ok(stream)?;
    stream.flush()?;
    let _ = std::fs::remove_file(&state.socket_path);
    std::process::exit(0);
}

fn handle_attach(state: &Arc<ServerState>, mut stream: UnixStream, name: &str) -> io::Result<()> {
    let session = {
        let sessions = state.sessions.lock().unwrap();
        sessions.get(name).cloned()
    };

    let Some(session) = session else {
        write_err(&mut stream, "missing session")?;
        return Ok(());
    };
    let Some(pane) = session.active_pane() else {
        write_err(&mut stream, "missing pane")?;
        return Ok(());
    };

    if has_attach_pane_snapshot(&session) {
        write_attach_snapshot_ok(&mut stream)?;
        return Ok(());
    }

    write_ok(&mut stream)?;
    {
        let history = pane.raw_history.lock().unwrap();
        stream.write_all(&history)?;
    }
    pane.clients.lock().unwrap().push(stream.try_clone()?);

    let mut buf = [0_u8; 8192];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        pane.writer.lock().unwrap().write_all(&buf[..n])?;
    }

    Ok(())
}

fn has_attach_pane_snapshot(session: &Session) -> bool {
    session.attach_panes().len() > 1
}

fn write_attach_snapshot_ok(stream: &mut UnixStream) -> io::Result<()> {
    stream.write_all(b"OK\tSNAPSHOT\n")
}

fn handle_attach_snapshot(
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
    if let Some(snapshot) = attach_pane_snapshot(&session) {
        stream.write_all(snapshot.as_bytes())?;
    }
    Ok(())
}

fn attach_pane_snapshot(session: &Session) -> Option<String> {
    let panes = session.attach_panes();
    if panes.len() <= 1 {
        return None;
    }

    let snapshots = panes
        .into_iter()
        .map(|pane| PaneSnapshot {
            index: pane.index,
            screen: capture_pane_text(&pane.pane, CaptureMode::Screen),
        })
        .collect::<Vec<_>>();

    Some(render_attach_pane_snapshot(&snapshots))
}

fn render_attach_pane_snapshot(panes: &[PaneSnapshot]) -> String {
    let mut output = String::new();
    for pane in panes {
        output.push_str("\r\n-- pane ");
        output.push_str(&pane.index.to_string());
        output.push_str(" --\r\n");
        for line in pane.screen.lines() {
            output.push_str(line);
            output.push_str("\r\n");
        }
    }
    output
}

fn start_output_pump(mut reader: File, pane: Arc<Pane>) {
    std::thread::spawn(move || {
        let mut buf = [0_u8; 8192];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            let bytes = &buf[..n];
            append_history(&pane.raw_history, bytes);
            pane.terminal.lock().unwrap().apply_bytes(bytes);
            broadcast(&pane.clients, bytes);
        }
    });
}

fn append_history(history: &Mutex<Vec<u8>>, bytes: &[u8]) {
    let mut history = history.lock().unwrap();
    history.extend_from_slice(bytes);
    if history.len() > MAX_HISTORY_BYTES {
        let excess = history.len() - MAX_HISTORY_BYTES;
        history.drain(..excess);
    }
}

fn broadcast(clients: &Mutex<Vec<UnixStream>>, bytes: &[u8]) {
    let mut clients = clients.lock().unwrap();
    let mut live = Vec::with_capacity(clients.len());

    for mut client in clients.drain(..) {
        if client.write_all(bytes).is_ok() {
            live.push(client);
        }
    }

    *clients = live;
}

fn write_ok(stream: &mut UnixStream) -> io::Result<()> {
    stream.write_all(b"OK\n")
}

fn write_err(stream: &mut UnixStream, message: &str) -> io::Result<()> {
    writeln!(stream, "ERR {message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_set_removes_active_and_selects_previous_pane() {
        let mut panes = PaneSet::new("base");
        panes.add("split");

        let removed = panes.kill(None).unwrap();

        assert_eq!(removed, "split");
        assert_eq!(panes.active(), Some("base"));
        assert_eq!(panes.len(), 1);
    }

    #[test]
    fn pane_set_rejects_last_pane_removal() {
        let mut panes = PaneSet::new("base");

        assert_eq!(
            panes.kill(None),
            Err("cannot kill last pane; use kill-session")
        );
        assert_eq!(panes.active(), Some("base"));
    }

    #[test]
    fn layout_node_splits_active_leaf_horizontally() {
        let mut layout = LayoutNode::Pane(0);

        assert!(layout.split_pane(0, SplitDirection::Horizontal, 1));

        assert_eq!(
            layout,
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                first: Box::new(LayoutNode::Pane(0)),
                second: Box::new(LayoutNode::Pane(1)),
            }
        );
    }

    #[test]
    fn layout_node_removes_pane_and_shifts_remaining_indexes() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                first: Box::new(LayoutNode::Pane(1)),
                second: Box::new(LayoutNode::Pane(2)),
            }),
        };

        assert!(layout.remove_pane(1));

        assert_eq!(
            layout,
            LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                first: Box::new(LayoutNode::Pane(0)),
                second: Box::new(LayoutNode::Pane(1)),
            }
        );
    }

    #[test]
    fn buffer_store_evicts_oldest_buffer_at_capacity() {
        let mut store = BufferStore::new();

        for index in 0..=MAX_BUFFERS {
            store
                .save(Some(&format!("buffer-{index}")), format!("text-{index}"))
                .unwrap();
        }

        assert!(store.resolve(Some("buffer-0")).is_none());
        assert!(
            store
                .resolve(Some(&format!("buffer-{MAX_BUFFERS}")))
                .is_some()
        );
        assert_eq!(store.list().len(), MAX_BUFFERS);
    }

    #[test]
    fn selected_buffer_text_returns_line_range() {
        let text = select_buffer_text(
            "first\nkeep-one\nkeep-two\nlast\n",
            &BufferSelection::LineRange { start: 2, end: 3 },
        )
        .unwrap();
        assert_eq!(text, "keep-one\nkeep-two\n");
    }

    #[test]
    fn selected_buffer_text_returns_first_search_match() {
        let text = select_buffer_text(
            "first\nneedle-one\nneedle-two\n",
            &BufferSelection::Search("needle".to_string()),
        )
        .unwrap();
        assert_eq!(text, "needle-one\n");
    }

    #[test]
    fn selected_buffer_text_rejects_missing_line_range() {
        let err = select_buffer_text(
            "first\nsecond\n",
            &BufferSelection::LineRange { start: 2, end: 3 },
        )
        .unwrap_err();
        assert_eq!(err, "missing line");
    }

    #[test]
    fn selected_buffer_text_rejects_missing_search_match() {
        let err = select_buffer_text(
            "first\nsecond\n",
            &BufferSelection::Search("needle".to_string()),
        )
        .unwrap_err();
        assert_eq!(err, "missing match");
    }

    #[test]
    fn format_copy_mode_lines_numbers_all_lines() {
        let text = format_copy_mode_lines("first\nsecond\n", None);
        assert_eq!(text, "1\tfirst\n2\tsecond\n");
    }

    #[test]
    fn format_copy_mode_lines_filters_search_matches() {
        let text = format_copy_mode_lines("first\nneedle-one\nlast\nneedle-two\n", Some("needle"));
        assert_eq!(text, "2\tneedle-one\n4\tneedle-two\n");
    }
}
