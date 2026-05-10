mod cli;
mod client;
mod paths;
mod protocol;
mod pty;
mod server;
mod term;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    match cli::parse_args(std::env::args())? {
        cli::Command::Server => server::run(paths::socket_path()).map_err(|err| err.to_string()),
        cli::Command::New {
            session,
            detach,
            command,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_new(&session, &command), true)?;
            if detach {
                Ok(())
            } else {
                Err("interactive attach is not implemented yet; use -d".to_string())
            }
        }
        cli::Command::ListSessions => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(&socket, protocol::encode_list(), true)?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::CapturePane { session } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(&socket, &protocol::encode_capture(&session), true)?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::ResizePane {
            session,
            cols,
            rows,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_resize(&session, cols, rows),
                true,
            )?;
            Ok(())
        }
        cli::Command::SendKeys { session, keys } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let bytes = encode_key_tokens(&keys)?;
            send_request(&socket, &protocol::encode_send(&session, &bytes), true)?;
            Ok(())
        }
        cli::Command::SplitWindow {
            session,
            direction,
            command,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_split(&session, direction, &command),
                true,
            )?;
            Ok(())
        }
        cli::Command::ListPanes { session } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(&socket, &protocol::encode_list_panes(&session), true)?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::SelectPane { session, pane } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_select_pane(&session, pane), true)?;
            Ok(())
        }
        cli::Command::KillPane { session, pane } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_kill_pane(&session, pane), true)?;
            Ok(())
        }
        cli::Command::NewWindow { session, command } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_new_window(&session, &command),
                true,
            )?;
            Ok(())
        }
        cli::Command::ListWindows { session } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(&socket, &protocol::encode_list_windows(&session), true)?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::SelectWindow { session, window } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_select_window(&session, window),
                true,
            )?;
            Ok(())
        }
        cli::Command::KillSession { session } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_kill(&session), true)?;
            Ok(())
        }
        cli::Command::KillServer => {
            let socket = paths::socket_path();
            if !socket.exists() {
                return Ok(());
            }
            send_request(&socket, protocol::encode_kill_server(), false)?;
            Ok(())
        }
        cli::Command::Attach { session } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let initial_size = client::detect_attach_size();
            if let Some(size) = initial_size {
                send_request(
                    &socket,
                    &protocol::encode_resize(&session, size.cols, size.rows),
                    true,
                )?;
            }
            client::attach(&socket, &session, initial_size, |size| {
                send_request(
                    &socket,
                    &protocol::encode_resize(&session, size.cols, size.rows),
                    true,
                )
                .map(|_| ())
                .map_err(io_error)
            })
            .map_err(|err| err.to_string())
        }
    }
}

fn encode_key_tokens(keys: &[String]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for key in keys {
        match key.as_str() {
            "Enter" => bytes.push(b'\r'),
            "Space" => bytes.push(b' '),
            "Tab" => bytes.push(b'\t'),
            "Escape" => bytes.push(0x1b),
            "C-c" => bytes.push(0x03),
            literal => bytes.extend_from_slice(literal.as_bytes()),
        }
    }
    Ok(bytes)
}

fn io_error(message: String) -> std::io::Error {
    std::io::Error::other(message)
}

fn ensure_server(socket: &std::path::Path) -> Result<(), String> {
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return Ok(());
    }

    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    if socket.exists() {
        std::fs::remove_file(socket).map_err(|err| err.to_string())?;
    }

    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    std::process::Command::new(exe)
        .arg("__server")
        .env("DEVMUX_SOCKET", socket)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("failed to start dmux server: {err}"))?;

    for _ in 0..100 {
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    Err(format!(
        "dmux server did not become ready at {}",
        socket.display()
    ))
}

fn send_request(
    socket: &std::path::Path,
    line: &str,
    spawn_if_missing: bool,
) -> Result<Vec<u8>, String> {
    if spawn_if_missing {
        ensure_server(socket)?;
    }

    let mut stream = std::os::unix::net::UnixStream::connect(socket)
        .map_err(|err| format!("failed to connect to {}: {err}", socket.display()))?;
    std::io::Write::write_all(&mut stream, line.as_bytes()).map_err(|err| err.to_string())?;

    let response = read_line(&mut stream).map_err(|err| err.to_string())?;
    if let Some(message) = response.strip_prefix("ERR ") {
        return Err(message.trim_end().to_string());
    }
    if response != "OK\n" {
        return Err(format!("unexpected server response: {response:?}"));
    }

    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut body).map_err(|err| err.to_string())?;
    Ok(body)
}

fn read_line(stream: &mut std::os::unix::net::UnixStream) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];

    loop {
        let n = std::io::Read::read(stream, &mut byte)?;
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
