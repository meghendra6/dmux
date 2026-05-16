mod cli;
mod client;
mod ids;
mod layout;
mod paths;
mod protocol;
mod pty;
mod server;
mod term;
mod terminal_query;

const DEFAULT_LIST_WINDOWS_FORMAT: &str = "#{window.index}\tid=#{window.id}\tname=#{window.name}\tactive=#{window.active}\tpanes=#{window.panes}";

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    match cli::parse_args(std::env::args())? {
        cli::Command::Server => server::run(paths::socket_path()).map_err(|err| err.to_string()),
        cli::Command::OpenDefault => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(&socket, protocol::encode_list(), true)?;
            let sessions = String::from_utf8_lossy(&body);
            if !sessions.lines().any(|line| line == "default") {
                match send_request(&socket, &protocol::encode_new("default", &[]), true) {
                    Ok(_) => {}
                    Err(error) if is_duplicate_default_create_error(&error) => {}
                    Err(error) => return Err(error),
                }
            }
            attach_session(&socket, "default")
        }
        cli::Command::Help { topic } => {
            match topic {
                Some(cli::HelpTopic::Attach) => print!("{}", cli::attach_help()),
                None => print!("{}", cli::general_help()),
            }
            Ok(())
        }
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
                attach_session(&socket, &session)
            }
        }
        cli::Command::ListSessions { format } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            let request = protocol::encode_list_sessions(format.as_deref());
            let body = send_request(&socket, &request, false)?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::RenameSession { old_name, new_name } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_rename_session(&old_name, &new_name),
                false,
            )?;
            Ok(())
        }
        cli::Command::ListClients { session, format } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            let body = send_request(
                &socket,
                &protocol::encode_list_clients(session.as_deref(), format.as_deref()),
                false,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::DetachClient { session, client_id } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_detach_client(session.as_deref(), client_id),
                false,
            )?;
            Ok(())
        }
        cli::Command::CapturePane {
            target,
            mode,
            selection,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(
                &socket,
                &protocol::encode_capture_target(&target, mode, selection),
                true,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::SaveBuffer {
            target,
            buffer,
            mode,
            selection,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(
                &socket,
                &protocol::encode_save_buffer_target(&target, buffer.as_deref(), mode, selection),
                true,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::CopyMode {
            session,
            mode,
            search,
            match_index,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(
                &socket,
                &protocol::encode_copy_mode(&session, mode, search.as_deref(), match_index),
                true,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::ListBuffers { format } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let request = protocol::encode_list_buffers(format.as_deref());
            let body = send_request(&socket, &request, true)?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::PasteBuffer { target, buffer } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_paste_buffer_target(&target, buffer.as_deref()),
                true,
            )?;
            Ok(())
        }
        cli::Command::DeleteBuffer { buffer } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_delete_buffer(&buffer), true)?;
            Ok(())
        }
        cli::Command::ResizePane { target, resize } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let request = match resize {
                cli::PaneResize::Absolute { cols, rows } => {
                    protocol::encode_resize(&target.session, cols, rows)
                }
                cli::PaneResize::Directional { direction, amount } => {
                    protocol::encode_resize_pane_target(&target, direction, amount)
                }
            };
            send_request(&socket, &request, true)?;
            Ok(())
        }
        cli::Command::SendKeys { target, keys } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let bytes = encode_key_tokens(&keys)?;
            send_request(
                &socket,
                &protocol::encode_send_target(&target, &bytes),
                true,
            )?;
            Ok(())
        }
        cli::Command::SplitWindow {
            target,
            direction,
            command,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_split_target(&target, direction, &command),
                true,
            )?;
            Ok(())
        }
        cli::Command::ListPanes {
            session,
            window,
            format,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(
                &socket,
                &protocol::encode_list_panes_target(&session, window, format.as_deref()),
                true,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::SelectPane {
            session,
            window,
            target,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_select_pane_in_window(&session, window, target),
                true,
            )?;
            Ok(())
        }
        cli::Command::KillPane { target } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_kill_pane_target(&target), true)?;
            Ok(())
        }
        cli::Command::RespawnPane {
            target,
            force,
            command,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_respawn_pane_target(&target, force, &command),
                true,
            )?;
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
        cli::Command::ListWindows { session, format } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let format = format.as_deref().unwrap_or(DEFAULT_LIST_WINDOWS_FORMAT);
            let body = send_request(
                &socket,
                &protocol::encode_list_windows(&session, Some(format)),
                true,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::SelectWindow { session, target } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_select_window_target(&session, target),
                true,
            )?;
            Ok(())
        }
        cli::Command::RenameWindow {
            session,
            target,
            name,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_rename_window(&session, target, &name),
                true,
            )?;
            Ok(())
        }
        cli::Command::NextWindow { session } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_next_window(&session), true)?;
            Ok(())
        }
        cli::Command::PreviousWindow { session } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_previous_window(&session), true)?;
            Ok(())
        }
        cli::Command::KillWindow { session, target } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_kill_window_target(&session, target),
                true,
            )?;
            Ok(())
        }
        cli::Command::ZoomPane { target } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_zoom_pane_target(&target), true)?;
            Ok(())
        }
        cli::Command::StatusLine { session, format } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(
                &socket,
                &protocol::encode_status_line(&session, format.as_deref()),
                true,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::DisplayMessage { session, format } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(
                &socket,
                &protocol::encode_display_message(&session, &format),
                true,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::KillSession { session } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(&socket, &protocol::encode_kill(&session), false)?;
            Ok(())
        }
        cli::Command::KillServer => {
            let socket = paths::socket_path();
            if !socket.exists() {
                return Ok(());
            }
            match send_request(&socket, protocol::encode_kill_server(), false) {
                Ok(_) => Ok(()),
                Err(error) if is_missing_socket_connect_error(&error) => Ok(()),
                Err(error) if is_stale_socket_connect_error(&error) => {
                    remove_stale_socket_path(&socket).map_err(|_| error)
                }
                Err(error) => Err(error),
            }
        }
        cli::Command::Attach { session } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            attach_session(&socket, &session)
        }
    }
}

fn require_running_server(socket: &std::path::Path) -> Result<(), String> {
    if std::os::unix::net::UnixStream::connect(socket).is_ok() {
        return Ok(());
    }

    Err("no dmux server running; create a session with dmux new -s <name>".to_string())
}

fn is_missing_socket_connect_error(error: &str) -> bool {
    error.starts_with("failed to connect to ") && error.contains("No such file or directory")
}

fn is_stale_socket_connect_error(error: &str) -> bool {
    error.starts_with("failed to connect to ") && error.contains("Connection refused")
}

fn is_duplicate_default_create_error(error: &str) -> bool {
    error == "session already exists; use dmux attach -t default"
}

fn remove_stale_socket_path(socket: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::FileTypeExt;

    let metadata = match std::fs::symlink_metadata(socket) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is not a socket",
        ));
    }
    std::fs::remove_file(socket)
}

fn attach_session(socket: &std::path::Path, session: &str) -> Result<(), String> {
    let initial_size = client::detect_attach_size();
    if let Some(size) = initial_size {
        send_request(
            socket,
            &protocol::encode_resize(session, size.cols, size.rows),
            true,
        )?;
    }
    client::attach(socket, session, initial_size, |size| {
        send_request(
            socket,
            &protocol::encode_resize(session, size.cols, size.rows),
            true,
        )
        .map(|_| ())
        .map_err(io_error)
    })
    .map_err(|err| err.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_default_create_error_is_ignorable_for_open_default() {
        assert!(is_duplicate_default_create_error(
            "session already exists; use dmux attach -t default"
        ));
        assert!(!is_duplicate_default_create_error(
            "session already exists; use dmux attach -t other"
        ));
        assert!(!is_duplicate_default_create_error("missing session"));
    }
}
