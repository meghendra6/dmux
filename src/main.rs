mod cli;
mod client;
mod config;
mod git;
mod ids;
mod json;
mod layout;
mod paths;
mod popup;
mod protocol;
mod pty;
mod registry;
mod server;
mod term;
mod terminal_query;

const DEFAULT_LIST_WINDOWS_FORMAT: &str = "#{window.index}\tid=#{window.id}\tname=#{window.name}\tactive=#{window.active}\tpanes=#{window.panes}";
/// Reserved `-F`/`--format` value that selects JSON output instead of a format
/// string (a bare `json` is never a useful `#{...}` format).
const JSON_FORMAT_SELECTOR: &str = "json";
/// Internal field-separated format requested from the server for JSON output;
/// the unit separator keeps fields unambiguous across names with whitespace.
const LIST_SESSIONS_JSON_FORMAT: &str = "#{session.name}\u{1f}#{session.window_count}\u{1f}#{session.attached_count}\u{1f}#{session.created_at}";
const LIST_PANES_JSON_FORMAT: &str = "#{pane.index}\u{1f}#{pane.id}\u{1f}#{pane.active}\u{1f}#{pane.zoomed}\u{1f}#{pane.state}\u{1f}#{pane.pid}\u{1f}#{pane.exit_status}\u{1f}#{pane.exit_signal}\u{1f}#{pane.title}\u{1f}#{pane.cwd}\u{1f}#{pane.bell}\u{1f}#{pane.activity}";
const LIST_WINDOWS_JSON_FORMAT: &str = "#{window.index}\u{1f}#{window.id}\u{1f}#{window.name}\u{1f}#{window.active}\u{1f}#{window.panes}\u{1f}#{window.bell}\u{1f}#{window.activity}";
const MAX_RUN_SHELL_OUTPUT_BYTES: usize = 64 * 1024;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let command = cli::parse_args(std::env::args())?;
    execute_command(command)
}

fn execute_command(command: cli::Command) -> Result<(), String> {
    match command {
        cli::Command::Server => server::run(paths::socket_path()).map_err(|err| err.to_string()),
        cli::Command::OpenDefault => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let body = send_request(&socket, protocol::encode_list(), true)?;
            let sessions = String::from_utf8_lossy(&body);
            if !sessions.lines().any(|line| line == "default") {
                let cwd = current_request_cwd()?;
                match send_request(
                    &socket,
                    &protocol::encode_new_in_cwd("default", &[], &cwd),
                    true,
                ) {
                    Ok(_) => {
                        let _ = registry::record_session(
                            &paths::workspace_registry_path(),
                            cwd,
                            "default",
                            "live",
                        );
                    }
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
            let cwd = current_request_cwd()?;
            send_request(
                &socket,
                &protocol::encode_new_in_cwd(&session, &command, &cwd),
                true,
            )?;
            let _ = registry::record_session(
                &paths::workspace_registry_path(),
                cwd,
                &session,
                if detach { "detached" } else { "live" },
            );
            if detach {
                Ok(())
            } else {
                attach_session(&socket, &session)
            }
        }
        cli::Command::ListSessions { format } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            if format.as_deref() == Some(JSON_FORMAT_SELECTOR) {
                let request = protocol::encode_list_sessions(Some(LIST_SESSIONS_JSON_FORMAT));
                let body = send_request(&socket, &request, false)?;
                println!("{}", list_sessions_json(&String::from_utf8_lossy(&body)));
                return Ok(());
            }
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
        cli::Command::SelectLayout {
            session,
            window,
            preset,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let target = protocol::Target {
                session,
                window,
                pane: protocol::PaneTarget::Active,
            };
            send_request(
                &socket,
                &protocol::encode_select_layout_target(&target, preset),
                true,
            )?;
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
            let cwd = current_request_cwd()?;
            send_request(
                &socket,
                &protocol::encode_split_target_in_cwd(&target, direction, &command, &cwd),
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
            if format.as_deref() == Some(JSON_FORMAT_SELECTOR) {
                let body = send_request(
                    &socket,
                    &protocol::encode_list_panes_target(
                        &session,
                        window,
                        Some(LIST_PANES_JSON_FORMAT),
                    ),
                    true,
                )?;
                println!("{}", list_panes_json(&String::from_utf8_lossy(&body)));
                return Ok(());
            }
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
        cli::Command::SwapPane {
            source,
            destination,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_swap_pane(&source, &destination),
                true,
            )?;
            Ok(())
        }
        cli::Command::MovePane {
            source,
            destination,
            direction,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_move_pane(&source, &destination, direction),
                true,
            )?;
            Ok(())
        }
        cli::Command::BreakPane { target } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(&socket, &protocol::encode_break_pane(&target), true)?;
            Ok(())
        }
        cli::Command::JoinPane {
            source,
            destination,
            direction,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_join_pane(&source, &destination, direction),
                true,
            )?;
            Ok(())
        }
        cli::Command::RespawnPane {
            target,
            force,
            command,
        } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let cwd = current_request_cwd()?;
            send_request(
                &socket,
                &protocol::encode_respawn_pane_target_in_cwd(&target, force, &command, &cwd),
                true,
            )?;
            Ok(())
        }
        cli::Command::NewWindow { session, command } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            let cwd = current_request_cwd()?;
            send_request(
                &socket,
                &protocol::encode_new_window_in_cwd(&session, &command, &cwd),
                true,
            )?;
            Ok(())
        }
        cli::Command::ListWindows { session, format } => {
            let socket = paths::socket_path();
            ensure_server(&socket)?;
            if format.as_deref() == Some(JSON_FORMAT_SELECTOR) {
                let body = send_request(
                    &socket,
                    &protocol::encode_list_windows(&session, Some(LIST_WINDOWS_JSON_FORMAT)),
                    true,
                )?;
                println!("{}", list_windows_json(&String::from_utf8_lossy(&body)));
                return Ok(());
            }
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
        cli::Command::MoveWindow { session, src, dst } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_move_window(&session, src, dst),
                false,
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
        cli::Command::WorkspaceAdd { path } => registry::register_workspace(
            &paths::workspace_registry_path(),
            std::path::PathBuf::from(path),
        )
        .map_err(|err| err.to_string()),
        cli::Command::WorkspaceList => {
            let registry =
                registry::load(&paths::workspace_registry_path()).map_err(|err| err.to_string())?;
            for workspace in registry.workspaces {
                println!("{}", workspace.path.display());
            }
            Ok(())
        }
        cli::Command::AgentEvent {
            target,
            state,
            label,
            source,
            changed_at,
        } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(
                &socket,
                &protocol::encode_agent_event(
                    &target,
                    &state,
                    &label,
                    source.as_deref(),
                    changed_at,
                ),
                false,
            )?;
            Ok(())
        }
        cli::Command::Notify {
            message,
            state,
            clear,
        } => {
            let pane_id = current_pane_id()?;
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            let request = if clear {
                protocol::encode_notify_clear(pane_id)
            } else {
                protocol::encode_notify(
                    pane_id,
                    state.as_deref().unwrap_or("needs_input"),
                    message.as_deref().unwrap_or(""),
                )
            };
            send_request(&socket, &request, false)?;
            Ok(())
        }
        cli::Command::ListAttention { session } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            let text = client::list_attention(&socket, session.as_deref())
                .map_err(|err| err.to_string())?;
            if !text.is_empty() {
                println!("{text}");
            }
            Ok(())
        }
        cli::Command::ListKeys { format } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            let body = send_request(
                &socket,
                &protocol::encode_list_keys(format.as_deref()),
                false,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::BindKey { key, command } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(&socket, &protocol::encode_bind_key(&key, &command), false)?;
            Ok(())
        }
        cli::Command::UnbindKey { key } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(&socket, &protocol::encode_unbind_key(&key), false)?;
            Ok(())
        }
        cli::Command::ShowOptions { format } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            let body = send_request(
                &socket,
                &protocol::encode_show_options(format.as_deref()),
                false,
            )?;
            print!("{}", String::from_utf8_lossy(&body));
            Ok(())
        }
        cli::Command::SetOption { name, value } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(&socket, &protocol::encode_set_option(&name, &value), false)?;
            Ok(())
        }
        cli::Command::Run { sequence } => execute_command_sequence(&sequence),
        cli::Command::SourceFile { path } => execute_command_file(&path),
        cli::Command::RunShell { command } => run_shell_command(&command),
        cli::Command::KillSession { session } => {
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(&socket, &protocol::encode_kill(&session), false)?;
            let _ = registry::mark_session_stopped(&paths::workspace_registry_path(), &session);
            Ok(())
        }
        cli::Command::HasSession { session } => {
            // tmux convention: exit 0 if the session exists, non-zero otherwise.
            let socket = paths::socket_path();
            require_running_server(&socket)?;
            send_request(&socket, &protocol::encode_has_session(&session), false)?;
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

fn execute_command_sequence(sequence: &str) -> Result<(), String> {
    let commands = cli::parse_command_sequence(sequence).map_err(|err| format!("run: {err}"))?;
    if commands.is_empty() {
        return Err("run requires at least one command".to_string());
    }

    for (index, command) in commands.iter().enumerate() {
        let context = format!("run command {}", index + 1);
        execute_script_command(command, &context)?;
    }
    Ok(())
}

fn execute_command_file(path: &str) -> Result<(), String> {
    let contents =
        std::fs::read_to_string(path).map_err(|err| format!("source-file {path:?}: {err}"))?;
    let commands =
        cli::parse_command_file(&contents).map_err(|err| format!("source-file {path:?}: {err}"))?;

    for entry in commands {
        let context = format!("source-file {path:?} line {}", entry.line);
        execute_script_command(&entry.command, &context)?;
    }
    Ok(())
}

fn execute_script_command(command: &cli::ScriptCommand, context: &str) -> Result<(), String> {
    let name = command
        .argv
        .first()
        .ok_or_else(|| format!("{context}: empty command"))?;
    let parsed = if name == "run-shell" {
        let shell_command = script_command_tail(&command.source, name)
            .ok_or_else(|| format!("{context} ({name}): run-shell requires a shell command"))?;
        if shell_command.trim().is_empty() {
            return Err(format!(
                "{context} ({name}): run-shell requires a shell command"
            ));
        }
        cli::Command::RunShell {
            command: shell_command.to_string(),
        }
    } else {
        cli::parse_args(std::iter::once("dmux".to_string()).chain(command.argv.iter().cloned()))
            .map_err(|err| format!("{context} ({name}): {err}"))?
    };

    if let Some(reason) = script_command_rejection(&parsed) {
        return Err(format!("{context} ({name}): {reason}"));
    }

    execute_command(parsed).map_err(|err| format!("{context} ({name}) failed: {err}"))
}

fn script_command_tail<'a>(source: &'a str, command_name: &str) -> Option<&'a str> {
    let source = source.trim_start();
    let tail = source.strip_prefix(command_name)?;
    if tail.is_empty() {
        return Some(tail);
    }
    if tail.chars().next().is_some_and(char::is_whitespace) {
        Some(tail.trim_start())
    } else {
        None
    }
}

fn script_command_rejection(command: &cli::Command) -> Option<&'static str> {
    match command {
        cli::Command::OpenDefault => Some("the default attach command is not allowed in scripts"),
        cli::Command::Server => Some("internal server command is not allowed in scripts"),
        cli::Command::Attach { .. } => Some("attach is interactive and is not allowed in scripts"),
        cli::Command::Help { .. } => Some("help is not allowed in scripts"),
        cli::Command::New { detach: false, .. } => {
            Some("new would attach interactively; use new -d in scripts")
        }
        cli::Command::Run { .. } => Some("nested run is not allowed in scripts"),
        cli::Command::SourceFile { .. } => Some("nested source-file is not allowed in scripts"),
        _ => None,
    }
}

fn run_shell_command(command: &str) -> Result<(), String> {
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("run-shell failed to start shell: {err}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "run-shell failed to capture stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "run-shell failed to capture stderr".to_string())?;
    let stdout_reader = std::thread::spawn(move || read_bounded_output(stdout));
    let stderr_reader = std::thread::spawn(move || read_bounded_output(stderr));
    let status = child
        .wait()
        .map_err(|err| format!("run-shell failed to wait for shell: {err}"))?;

    let stdout = join_bounded_output(stdout_reader, "stdout")?;
    let stderr = join_bounded_output(stderr_reader, "stderr")?;
    write_bounded_output(&stdout, true)?;
    write_bounded_output(&stderr, false)?;

    if status.success() {
        return Ok(());
    }

    Err(format!(
        "run-shell exited with {}",
        format_exit_status(status)
    ))
}

struct BoundedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn read_bounded_output<R: std::io::Read>(mut reader: R) -> std::io::Result<BoundedOutput> {
    let mut bytes = Vec::new();
    let mut total = 0usize;
    let mut buffer = [0_u8; 8192];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count);
        let remaining = MAX_RUN_SHELL_OUTPUT_BYTES.saturating_sub(bytes.len());
        if remaining > 0 {
            bytes.extend_from_slice(&buffer[..count.min(remaining)]);
        }
    }

    Ok(BoundedOutput {
        truncated: total > bytes.len(),
        bytes,
    })
}

fn join_bounded_output(
    reader: std::thread::JoinHandle<std::io::Result<BoundedOutput>>,
    stream: &str,
) -> Result<BoundedOutput, String> {
    reader
        .join()
        .map_err(|_| format!("run-shell {stream} reader panicked"))?
        .map_err(|err| format!("run-shell failed to read {stream}: {err}"))
}

fn write_bounded_output(output: &BoundedOutput, stdout: bool) -> Result<(), String> {
    if stdout {
        std::io::Write::write_all(&mut std::io::stdout(), &output.bytes)
    } else {
        std::io::Write::write_all(&mut std::io::stderr(), &output.bytes)
    }
    .map_err(|err| err.to_string())?;

    if output.truncated {
        let message = format!(
            "\n[dmux: run-shell output truncated after {MAX_RUN_SHELL_OUTPUT_BYTES} bytes]\n"
        );
        std::io::Write::write_all(&mut std::io::stderr(), message.as_bytes())
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn format_exit_status(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        format!("status {code}")
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return format!("signal {signal}");
            }
        }
        "unknown status".to_string()
    }
}

/// Resolve the calling pane's global id from the `$DMUX_PANE` env var that pane
/// children inherit (formatted `%<id>`). Returns a user-facing error when not
/// running inside a dmux pane.
fn current_pane_id() -> Result<usize, String> {
    let raw = std::env::var("DMUX_PANE").map_err(|_| {
        "dmux notify must run inside a dmux pane (DMUX_PANE is not set)".to_string()
    })?;
    raw.strip_prefix('%')
        .and_then(|id| id.parse::<usize>().ok())
        .ok_or_else(|| format!("dmux notify found a malformed DMUX_PANE value: {raw:?}"))
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

fn current_request_cwd() -> Result<std::path::PathBuf, String> {
    std::env::current_dir().map_err(|err| err.to_string())
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

/// Render the field-separated `LIST_SESSIONS_JSON_FORMAT` body as a JSON array
/// of session objects. Each non-empty line is one session; missing trailing
/// fields are treated as empty.
fn list_sessions_json(body: &str) -> String {
    let objects = body
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split('\u{1f}');
            let name = fields.next().unwrap_or_default();
            let windows = fields.next().unwrap_or_default();
            let attached = fields.next().unwrap_or_default();
            let created_at = fields.next().unwrap_or_default();
            format!(
                "  {{\"name\": {}, \"windows\": {}, \"attached_count\": {}, \"created_at\": {}}}",
                json::json_string(name),
                json::json_u64_or_string(windows),
                json::json_u64_or_string(attached),
                json::json_u64_or_string(created_at),
            )
        })
        .collect::<Vec<_>>();
    if objects.is_empty() {
        "[]".to_string()
    } else {
        format!("[\n{}\n]", objects.join(",\n"))
    }
}

/// Render the field-separated `LIST_PANES_JSON_FORMAT` body as a JSON array of
/// pane objects. Boolean flags become JSON booleans; counts/ids stay numeric.
fn list_panes_json(body: &str) -> String {
    let objects = body
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split('\u{1f}');
            let index = fields.next().unwrap_or_default();
            let id = fields.next().unwrap_or_default();
            let active = fields.next().unwrap_or_default();
            let zoomed = fields.next().unwrap_or_default();
            let state = fields.next().unwrap_or_default();
            let pid = fields.next().unwrap_or_default();
            let exit_status = fields.next().unwrap_or_default();
            let exit_signal = fields.next().unwrap_or_default();
            let title = fields.next().unwrap_or_default();
            let cwd = fields.next().unwrap_or_default();
            let bell = fields.next().unwrap_or_default();
            let activity = fields.next().unwrap_or_default();
            format!(
                "  {{\"index\": {}, \"id\": {}, \"active\": {}, \"zoomed\": {}, \"state\": {}, \"pid\": {}, \"exit_status\": {}, \"exit_signal\": {}, \"title\": {}, \"cwd\": {}, \"bell\": {}, \"activity\": {}}}",
                json::json_u64_or_string(index),
                json::json_u64_or_string(id),
                json::json_bool(active),
                json::json_bool(zoomed),
                json::json_string(state),
                json::json_u64_or_string(pid),
                json::json_u64_or_string(exit_status),
                json::json_u64_or_string(exit_signal),
                json::json_string(title),
                json::json_string(cwd),
                json::json_bool(bell),
                json::json_bool(activity),
            )
        })
        .collect::<Vec<_>>();
    if objects.is_empty() {
        "[]".to_string()
    } else {
        format!("[\n{}\n]", objects.join(",\n"))
    }
}

/// Render the field-separated `LIST_WINDOWS_JSON_FORMAT` body as a JSON array
/// of window objects.
fn list_windows_json(body: &str) -> String {
    let objects = body
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split('\u{1f}');
            let index = fields.next().unwrap_or_default();
            let id = fields.next().unwrap_or_default();
            let name = fields.next().unwrap_or_default();
            let active = fields.next().unwrap_or_default();
            let panes = fields.next().unwrap_or_default();
            let bell = fields.next().unwrap_or_default();
            let activity = fields.next().unwrap_or_default();
            format!(
                "  {{\"index\": {}, \"id\": {}, \"name\": {}, \"active\": {}, \"panes\": {}, \"bell\": {}, \"activity\": {}}}",
                json::json_u64_or_string(index),
                json::json_u64_or_string(id),
                json::json_string(name),
                json::json_bool(active),
                json::json_u64_or_string(panes),
                json::json_bool(bell),
                json::json_bool(activity),
            )
        })
        .collect::<Vec<_>>();
    if objects.is_empty() {
        "[]".to_string()
    } else {
        format!("[\n{}\n]", objects.join(",\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_sessions_json_builds_array_of_objects() {
        let body = "dev\u{1f}2\u{1f}1\u{1f}1000\nwork\u{1f}1\u{1f}0\u{1f}2000\n";
        let json = list_sessions_json(body);
        assert_eq!(
            json,
            "[\n  {\"name\": \"dev\", \"windows\": 2, \"attached_count\": 1, \"created_at\": 1000},\n  {\"name\": \"work\", \"windows\": 1, \"attached_count\": 0, \"created_at\": 2000}\n]"
        );
    }

    #[test]
    fn list_sessions_json_handles_empty_and_quotes_names() {
        assert_eq!(list_sessions_json(""), "[]");
        let body = "a\"b\u{1f}0\u{1f}0\u{1f}0\n";
        assert!(
            list_sessions_json(body).contains("\"name\": \"a\\\"b\""),
            "{}",
            list_sessions_json(body)
        );
    }

    #[test]
    fn list_panes_json_renders_flags_as_booleans() {
        let body = "0\u{1f}2\u{1f}1\u{1f}0\u{1f}running\u{1f}123\u{1f}\u{1f}\u{1f}vim\u{1f}/tmp\u{1f}0\u{1f}1\n";
        let json = list_panes_json(body);
        assert_eq!(
            json,
            "[\n  {\"index\": 0, \"id\": 2, \"active\": true, \"zoomed\": false, \"state\": \"running\", \"pid\": 123, \"exit_status\": \"\", \"exit_signal\": \"\", \"title\": \"vim\", \"cwd\": \"/tmp\", \"bell\": false, \"activity\": true}\n]"
        );
        assert_eq!(list_panes_json(""), "[]");
    }

    #[test]
    fn list_windows_json_renders_objects() {
        let body = "0\u{1f}5\u{1f}main\u{1f}1\u{1f}3\u{1f}1\u{1f}0\n";
        let json = list_windows_json(body);
        assert_eq!(
            json,
            "[\n  {\"index\": 0, \"id\": 5, \"name\": \"main\", \"active\": true, \"panes\": 3, \"bell\": true, \"activity\": false}\n]"
        );
        assert_eq!(list_windows_json(""), "[]");
    }

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

    #[test]
    fn script_command_tail_preserves_shell_quoting() {
        assert_eq!(
            script_command_tail("run-shell printf '%s\\n' 'hello world'", "run-shell"),
            Some("printf '%s\\n' 'hello world'")
        );
    }

    #[test]
    fn bounded_output_reader_retains_cap_while_draining_extra_bytes() {
        let input = vec![b'x'; MAX_RUN_SHELL_OUTPUT_BYTES + 1024];
        let output = read_bounded_output(std::io::Cursor::new(input)).unwrap();

        assert_eq!(output.bytes.len(), MAX_RUN_SHELL_OUTPUT_BYTES);
        assert!(output.truncated);
    }
}
