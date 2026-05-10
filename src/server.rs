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
    let spec = SpawnSpec::new(name.clone(), command, cwd);
    let process = pty::spawn(&spec)?;
    let reader = process.master.try_clone()?;
    let session = Arc::new(Session {
        child_pid: process.child_pid,
        writer: Arc::new(Mutex::new(process.master)),
        size: Mutex::new(spec.size),
        raw_history: Arc::new(Mutex::new(Vec::new())),
        terminal: Arc::new(Mutex::new(TerminalState::new(80, 24, 10_000))),
        clients: Arc::new(Mutex::new(Vec::new())),
    });

    start_output_pump(reader, Arc::clone(&session));
    sessions.insert(name, session);
    write_ok(stream)
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

    write_ok(stream)?;
    let captured = session.terminal.lock().unwrap().capture_text();
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

    {
        let writer = session.writer.lock().unwrap();
        pty::resize(&writer, size)?;
    }
    *session.size.lock().unwrap() = size;
    session
        .terminal
        .lock()
        .unwrap()
        .resize(size.cols as usize, size.rows as usize);

    write_ok(stream)
}

fn handle_kill(state: &Arc<ServerState>, stream: &mut UnixStream, name: &str) -> io::Result<()> {
    let session = state.sessions.lock().unwrap().remove(name);
    let Some(session) = session else {
        write_err(stream, "missing session")?;
        return Ok(());
    };

    pty::terminate(session.child_pid);
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
        pty::terminate(session.child_pid);
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

    write_ok(&mut stream)?;
    {
        let history = session.raw_history.lock().unwrap();
        stream.write_all(&history)?;
    }
    session.clients.lock().unwrap().push(stream.try_clone()?);

    let mut buf = [0_u8; 8192];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        session.writer.lock().unwrap().write_all(&buf[..n])?;
    }

    Ok(())
}

fn start_output_pump(mut reader: File, session: Arc<Session>) {
    std::thread::spawn(move || {
        let mut buf = [0_u8; 8192];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            let bytes = &buf[..n];
            append_history(&session.raw_history, bytes);
            session.terminal.lock().unwrap().apply_bytes(bytes);
            broadcast(&session.clients, bytes);
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
