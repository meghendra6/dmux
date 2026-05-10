use crate::protocol::{self, Request};
use crate::pty::{self, PtySize, SpawnSpec};
use crate::term::TerminalState;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const MAX_HISTORY_BYTES: usize = 1024 * 1024;

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
    socket_path: PathBuf,
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
}

struct Window {
    panes: PaneSet<Arc<Pane>>,
    zoomed: Option<usize>,
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
        Request::Capture { session } => handle_capture(&state, &mut stream, &session),
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
        Request::Kill { session } => handle_kill(&state, &mut stream, &session),
        Request::KillServer => handle_kill_server(&state, &mut stream),
        Request::Attach { session } => handle_attach(&state, stream, &session),
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

fn handle_capture(state: &Arc<ServerState>, stream: &mut UnixStream, name: &str) -> io::Result<()> {
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
    let captured = pane.terminal.lock().unwrap().capture_text();
    stream.write_all(captured.as_bytes())
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
}
