use crate::protocol::{
    BufferSelection, CaptureMode, PaneDirection, PaneResizeDirection, PaneSelectTarget,
    SplitDirection, WindowTarget,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpTopic {
    Attach,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneResize {
    Absolute {
        cols: u16,
        rows: u16,
    },
    Directional {
        direction: PaneResizeDirection,
        amount: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    OpenDefault,
    New {
        session: String,
        detach: bool,
        command: Vec<String>,
    },
    Attach {
        session: String,
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
    CapturePane {
        session: String,
        mode: CaptureMode,
        selection: BufferSelection,
    },
    SaveBuffer {
        session: String,
        buffer: Option<String>,
        mode: CaptureMode,
        selection: BufferSelection,
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
        session: String,
        buffer: Option<String>,
    },
    DeleteBuffer {
        buffer: String,
    },
    ResizePane {
        session: String,
        resize: PaneResize,
    },
    SendKeys {
        session: String,
        keys: Vec<String>,
    },
    SplitWindow {
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
    KillSession {
        session: String,
    },
    KillServer,
    Help {
        topic: Option<HelpTopic>,
    },
    Server,
}

pub fn parse_args<I, S>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if args.is_empty() {
        return Ok(Command::OpenDefault);
    }

    let program = args.remove(0);
    let Some(subcommand) = args.first().cloned() else {
        return Ok(Command::OpenDefault);
    };
    args.remove(0);

    match subcommand.as_str() {
        "__server" => Ok(Command::Server),
        "-h" | "--help" => Ok(Command::Help { topic: None }),
        "help" => parse_help(args),
        "new" | "new-session" => parse_new(args),
        "attach" | "attach-session" => parse_attach(args),
        "ls" => parse_list_sessions(args, "ls"),
        "list-sessions" => parse_list_sessions(args, "list-sessions"),
        "rename-session" => parse_rename_session(args),
        "list-clients" => parse_list_clients(args),
        "detach-client" => parse_detach_client(args),
        "capture-pane" => parse_capture(args),
        "save-buffer" => parse_save_buffer(args),
        "copy-mode" => parse_copy_mode(args),
        "list-buffers" => parse_list_buffers(args),
        "paste-buffer" => parse_paste_buffer(args),
        "delete-buffer" => parse_delete_buffer(args),
        "resize-pane" => parse_resize_pane(args),
        "send-keys" => parse_send_keys(args),
        "split-window" | "split" => parse_split_window(args),
        "list-panes" => parse_list_panes(args),
        "select-pane" => parse_select_pane(args),
        "kill-pane" => parse_kill_pane(args),
        "respawn-pane" => parse_respawn_pane(args),
        "new-window" => parse_new_window(args, "new-window"),
        "new-tab" => parse_new_window(args, "new-tab"),
        "list-windows" => parse_list_windows(args, "list-windows"),
        "list-tabs" => parse_list_windows(args, "list-tabs"),
        "select-window" => parse_select_window(args, "select-window", "-w", &["-w"], "--window-id"),
        "select-tab" => {
            parse_select_window(args, "select-tab", "-i", &["-i", "--index"], "--tab-id")
        }
        "rename-window" => parse_rename_window(args, "rename-window", &["-w"], "--window-id"),
        "rename-tab" => parse_rename_window(args, "rename-tab", &["-i", "--index"], "--tab-id"),
        "next-window" => parse_cycle_window(args, "next-window", true),
        "next-tab" => parse_cycle_window(args, "next-tab", true),
        "previous-window" => parse_cycle_window(args, "previous-window", false),
        "previous-tab" => parse_cycle_window(args, "previous-tab", false),
        "kill-window" => parse_kill_window(args, "kill-window", &["-w"]),
        "kill-tab" => parse_kill_window(args, "kill-tab", &["-i", "--index"]),
        "zoom-pane" => parse_zoom_pane(args),
        "status-line" => parse_status_line(args),
        "display-message" => parse_display_message(args),
        "kill-session" => parse_kill_session(args),
        "kill-server" => Ok(Command::KillServer),
        _ => Err(format!("{program}: unknown command {subcommand:?}")),
    }
}

fn parse_help(args: Vec<String>) -> Result<Command, String> {
    match args.as_slice() {
        [] => Ok(Command::Help { topic: None }),
        [topic] if topic == "attach" || topic == "attach-session" => Ok(Command::Help {
            topic: Some(HelpTopic::Attach),
        }),
        [topic] => Err(format!("unknown help topic {topic:?}")),
        _ => Err("help accepts at most one topic".to_string()),
    }
}

fn parse_new(args: Vec<String>) -> Result<Command, String> {
    let mut detach = false;
    let mut session = None;
    let mut command = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-d" => {
                detach = true;
                i += 1;
            }
            "-s" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "new requires a session name after -s".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "--" => {
                command.extend(args[i + 1..].iter().cloned());
                break;
            }
            value if value.starts_with('-') => {
                return Err(format!("new does not support option {value:?}"));
            }
            _ => {
                command.extend(args[i..].iter().cloned());
                break;
            }
        }
    }

    Ok(Command::New {
        session: session.unwrap_or_else(|| "default".to_string()),
        detach,
        command,
    })
}

fn parse_attach(args: Vec<String>) -> Result<Command, String> {
    if matches!(args.as_slice(), [arg] if arg == "-h" || arg == "--help") {
        return Ok(Command::Help {
            topic: Some(HelpTopic::Attach),
        });
    }

    Ok(Command::Attach {
        session: parse_target(args, "attach")?.unwrap_or_else(|| "default".to_string()),
    })
}

pub fn general_help() -> &'static str {
    "Usage: dmux <command> [options]\n\
\n\
Commands:\n\
  new [-d] -s <name> [-- command...]    create a session; attach unless -d is used\n\
  attach [-t <name>]                    attach to a session; defaults to default\n\
  ls                                    list sessions\n\
  split-window -t <name> -h|-v [-- command...]\n\
  list-panes -t <name> [-F <format>]\n\
  select-pane -t <name> -p <index>|--pane-id <id>|-L|-R|-U|-D\n\
  respawn-pane -t <name> [-p <index>] [-k] [-- command...]\n\
  new-window -t <name> [-- command...]\n\
  list-windows -t <name> [-F <format>]\n\
  select-window -t <name> -w <index>|--window-id <id>|-n <name>\n\
  rename-window -t <name> [-w <index>|--window-id <id>|-n <old-name>] <new-name>\n\
  next-window -t <name>                  cycle to next window\n\
  previous-window -t <name>              cycle to previous window\n\
  new-tab -t <name> [-- command...]      alias for new-window\n\
  list-tabs -t <name> [-F <format>]      alias for list-windows\n\
  select-tab -t <name> -i <index>|--tab-id <id>|-n <name>\n\
  rename-tab -t <name> [-i <index>|--tab-id <id>|-n <old-name>] <new-name>\n\
  next-tab/previous-tab -t <name>        aliases for window cycling\n\
  kill-tab -t <name> [-i <index>]        alias for kill-window\n\
  kill-session -t <name>\n\
  kill-server\n\
\n\
Help:\n\
  dmux help attach\n\
  dmux attach --help\n"
}

pub fn attach_help() -> &'static str {
    "Usage: dmux attach [-t <name>]\n\
\n\
If -t is omitted, attach targets default.\n\
\n\
Attach keys:\n\
  C-b d       detach\n\
  C-b c       create a new window\n\
  C-b n/p     next/previous window\n\
  C-b %       split right\n\
  C-b \"       split down\n\
  C-b ?       show this help\n\
  C-b [       copy-mode for the active pane\n\
  C-b o       cycle panes in multi-pane attach\n\
  C-b h/j/k/l focus left/down/up/right\n\
  C-b H/J/K/L resize active pane left/down/up/right by 5 cells\n\
  C-b q       show pane numbers; press a digit to select\n\
  C-b x       close the active pane\n\
  C-b z       toggle zoom for the active pane\n\
  C-b C-b     send a literal prefix\n\
  mouse click focus a pane in unzoomed multi-pane attach\n\
\n\
Pane commands:\n\
  dmux split-window -t <name> -h [-- command...]  split left/right\n\
  dmux split-window -t <name> -v [-- command...]  split top/bottom\n\
  dmux resize-pane -t <name> -L|-R|-U|-D [amount]\n\
  dmux select-pane -t <name> -p <index>|--pane-id <id>|-L|-R|-U|-D\n"
}

pub fn attach_help_summary() -> &'static str {
    "C-b d detach / C-b D detach | C-b ? help | C-b c new window | C-b n/p next/previous window | C-b % split right | C-b \" split down | C-b h/j/k/l focus | C-b H/J/K/L resize by 5 | C-b x close | C-b z zoom | C-b [ copy-mode | C-b o next pane | C-b q pane numbers | C-b C-b literal prefix | mouse click focus pane | split: dmux split-window -t <name> -h|-v | resize: dmux resize-pane -t <name> -L|-R|-U|-D [amount] | select: dmux select-pane -t <name> -p <index>"
}

fn parse_list_sessions(args: Vec<String>, command_name: &str) -> Result<Command, String> {
    let mut format = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-F" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a format after -F"))?;
                format = Some(value.clone());
                i += 2;
            }
            value => {
                return Err(format!(
                    "{command_name} does not support argument {value:?}"
                ));
            }
        }
    }
    Ok(Command::ListSessions { format })
}

fn parse_rename_session(args: Vec<String>) -> Result<Command, String> {
    let mut old_name = None;
    let mut new_name = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "rename-session requires a session name after -t".to_string())?;
                old_name = Some(value.clone());
                i += 2;
            }
            value if value.starts_with('-') => {
                return Err(format!("rename-session does not support option {value:?}"));
            }
            value => {
                if new_name.replace(value.to_string()).is_some() {
                    return Err("rename-session accepts exactly one new name".to_string());
                }
                i += 1;
            }
        }
    }
    let new_name = new_name.ok_or_else(|| "rename-session requires <new-name>".to_string())?;
    if new_name.is_empty() {
        return Err("rename-session new name cannot be empty".to_string());
    }
    Ok(Command::RenameSession {
        old_name: old_name.ok_or_else(|| "rename-session requires -t <session>".to_string())?,
        new_name,
    })
}

fn parse_list_clients(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut format = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "list-clients requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-F" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "list-clients requires a format after -F".to_string())?;
                format = Some(value.clone());
                i += 2;
            }
            value => return Err(format!("list-clients does not support argument {value:?}")),
        }
    }
    Ok(Command::ListClients { session, format })
}

fn parse_detach_client(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut client_id = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "detach-client requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-c" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "detach-client requires a client id after -c".to_string())?;
                client_id =
                    Some(value.parse::<usize>().map_err(|_| {
                        "detach-client -c must be a non-negative integer".to_string()
                    })?);
                i += 2;
            }
            value => return Err(format!("detach-client does not support argument {value:?}")),
        }
    }
    if session.is_none() && client_id.is_none() {
        return Err("detach-client requires -t <session> or -c <client-id>".to_string());
    }
    Ok(Command::DetachClient { session, client_id })
}

fn parse_capture(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut mode = None;
    let mut start_line = None;
    let mut end_line = None;
    let mut search = None;
    let mut match_index = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "capture-pane requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-p" => {
                i += 1;
            }
            "--screen" => {
                set_capture_mode(&mut mode, CaptureMode::Screen)?;
                i += 1;
            }
            "--history" => {
                set_capture_mode(&mut mode, CaptureMode::History)?;
                i += 1;
            }
            "--all" => {
                set_capture_mode(&mut mode, CaptureMode::All)?;
                i += 1;
            }
            "--start-line" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "capture-pane requires a line number after --start-line".to_string()
                })?;
                start_line = Some(parse_line_offset(value, "capture-pane --start-line")?);
                i += 2;
            }
            "--end-line" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "capture-pane requires a line number after --end-line".to_string()
                })?;
                end_line = Some(parse_line_offset(value, "capture-pane --end-line")?);
                i += 2;
            }
            "--search" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "capture-pane requires text after --search".to_string())?;
                if value.is_empty() {
                    return Err("capture-pane --search requires non-empty text".to_string());
                }
                search = Some(value.clone());
                i += 2;
            }
            "--match" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "capture-pane requires a match index after --match".to_string()
                })?;
                match_index = Some(parse_positive_usize(value, "capture-pane --match")?);
                i += 2;
            }
            value => return Err(format!("capture-pane does not support argument {value:?}")),
        }
    }

    let selection =
        parse_buffer_selection(start_line, end_line, search, match_index, "capture-pane")?;

    Ok(Command::CapturePane {
        session: session.ok_or_else(|| "capture-pane requires -t <session>".to_string())?,
        mode: mode.unwrap_or(CaptureMode::All),
        selection,
    })
}

fn set_capture_mode(mode: &mut Option<CaptureMode>, value: CaptureMode) -> Result<(), String> {
    if mode.replace(value).is_some() {
        Err("only one capture mode may be supplied".to_string())
    } else {
        Ok(())
    }
}

fn parse_save_buffer(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut buffer = None;
    let mut mode = None;
    let mut start_line = None;
    let mut end_line = None;
    let mut search = None;
    let mut match_index = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "save-buffer requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-b" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "save-buffer requires a buffer name after -b".to_string())?;
                buffer = Some(parse_buffer_name(value, "save-buffer")?);
                i += 2;
            }
            "--screen" => {
                set_capture_mode(&mut mode, CaptureMode::Screen)?;
                i += 1;
            }
            "--history" => {
                set_capture_mode(&mut mode, CaptureMode::History)?;
                i += 1;
            }
            "--all" => {
                set_capture_mode(&mut mode, CaptureMode::All)?;
                i += 1;
            }
            "--start-line" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "save-buffer requires a line number after --start-line".to_string()
                })?;
                start_line = Some(parse_line_offset(value, "save-buffer --start-line")?);
                i += 2;
            }
            "--end-line" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "save-buffer requires a line number after --end-line".to_string()
                })?;
                end_line = Some(parse_line_offset(value, "save-buffer --end-line")?);
                i += 2;
            }
            "--search" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "save-buffer requires text after --search".to_string())?;
                if value.is_empty() {
                    return Err("save-buffer --search requires non-empty text".to_string());
                }
                search = Some(value.clone());
                i += 2;
            }
            "--match" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "save-buffer requires a match index after --match".to_string()
                })?;
                match_index = Some(parse_positive_usize(value, "save-buffer --match")?);
                i += 2;
            }
            value => return Err(format!("save-buffer does not support argument {value:?}")),
        }
    }

    let selection =
        parse_buffer_selection(start_line, end_line, search, match_index, "save-buffer")?;

    Ok(Command::SaveBuffer {
        session: session.ok_or_else(|| "save-buffer requires -t <session>".to_string())?,
        buffer,
        mode: mode.unwrap_or(CaptureMode::All),
        selection,
    })
}

fn parse_copy_mode(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut mode = None;
    let mut search = None;
    let mut match_index = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "copy-mode requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "--screen" => {
                set_capture_mode(&mut mode, CaptureMode::Screen)?;
                i += 1;
            }
            "--history" => {
                set_capture_mode(&mut mode, CaptureMode::History)?;
                i += 1;
            }
            "--all" => {
                set_capture_mode(&mut mode, CaptureMode::All)?;
                i += 1;
            }
            "--search" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "copy-mode requires text after --search".to_string())?;
                if value.is_empty() {
                    return Err("copy-mode --search requires non-empty text".to_string());
                }
                search = Some(value.clone());
                i += 2;
            }
            "--match" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "copy-mode requires a match index after --match".to_string())?;
                match_index = Some(parse_positive_usize(value, "copy-mode --match")?);
                i += 2;
            }
            value => return Err(format!("copy-mode does not support argument {value:?}")),
        }
    }
    if match_index.is_some() && search.is_none() {
        return Err("copy-mode --match requires --search".to_string());
    }

    Ok(Command::CopyMode {
        session: session.ok_or_else(|| "copy-mode requires -t <session>".to_string())?,
        mode: mode.unwrap_or(CaptureMode::All),
        search,
        match_index,
    })
}

fn parse_buffer_selection(
    start_line: Option<isize>,
    end_line: Option<isize>,
    search: Option<String>,
    match_index: Option<usize>,
    command: &str,
) -> Result<BufferSelection, String> {
    if search.is_some() && (start_line.is_some() || end_line.is_some()) {
        return Err(format!("{command} accepts either a line range or --search"));
    }

    if let Some(search) = search {
        return Ok(BufferSelection::Search {
            needle: search,
            match_index: match_index.unwrap_or(1),
        });
    }

    if match_index.is_some() {
        return Err(format!("{command} --match requires --search"));
    }

    parse_line_range_selection(start_line, end_line, command)
}

fn parse_line_range_selection(
    start_line: Option<isize>,
    end_line: Option<isize>,
    command: &str,
) -> Result<BufferSelection, String> {
    match (start_line, end_line) {
        (None, None) => Ok(BufferSelection::All),
        (Some(start), Some(end)) => Ok(BufferSelection::LineRange { start, end }),
        _ => Err(format!(
            "{command} line range requires --start-line and --end-line"
        )),
    }
}

fn parse_list_buffers(args: Vec<String>) -> Result<Command, String> {
    let mut format = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-F" | "--format" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "list-buffers requires a format after --format".to_string())?;
                format = Some(value.clone());
                i += 2;
            }
            value => return Err(format!("list-buffers does not support argument {value:?}")),
        }
    }

    Ok(Command::ListBuffers { format })
}

fn parse_paste_buffer(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut buffer = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "paste-buffer requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-b" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "paste-buffer requires a buffer name after -b".to_string())?;
                buffer = Some(parse_buffer_name(value, "paste-buffer")?);
                i += 2;
            }
            value => return Err(format!("paste-buffer does not support argument {value:?}")),
        }
    }

    Ok(Command::PasteBuffer {
        session: session.ok_or_else(|| "paste-buffer requires -t <session>".to_string())?,
        buffer,
    })
}

fn parse_delete_buffer(args: Vec<String>) -> Result<Command, String> {
    let mut buffer = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-b" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "delete-buffer requires a buffer name after -b".to_string())?;
                buffer = Some(parse_buffer_name(value, "delete-buffer")?);
                i += 2;
            }
            value => return Err(format!("delete-buffer does not support argument {value:?}")),
        }
    }

    Ok(Command::DeleteBuffer {
        buffer: buffer.ok_or_else(|| "delete-buffer requires -b <buffer>".to_string())?,
    })
}

fn parse_buffer_name(value: &str, command: &str) -> Result<String, String> {
    if value.is_empty() {
        Err(format!("{command} -b requires a non-empty buffer name"))
    } else {
        Ok(value.to_string())
    }
}

fn parse_positive_usize(value: &str, label: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{label} must be a positive integer"))
}

fn parse_line_offset(value: &str, label: &str) -> Result<isize, String> {
    value
        .parse::<isize>()
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| format!("{label} must be a non-zero integer"))
}

fn parse_kill_session(args: Vec<String>) -> Result<Command, String> {
    let session = parse_target(args, "kill-session")?
        .ok_or_else(|| "kill-session requires -t <session>".to_string())?;
    Ok(Command::KillSession { session })
}

fn parse_resize_pane(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut cols = None;
    let mut rows = None;
    let mut directional = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "resize-pane requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-x" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "resize-pane requires columns after -x".to_string())?;
                cols = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| "resize-pane -x must be a positive integer".to_string())?,
                );
                i += 2;
            }
            "-y" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "resize-pane requires rows after -y".to_string())?;
                rows = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| "resize-pane -y must be a positive integer".to_string())?,
                );
                i += 2;
            }
            "-L" | "-R" | "-U" | "-D" => {
                if directional.is_some() {
                    return Err("resize-pane accepts only one direction".to_string());
                }
                let direction = match args[i].as_str() {
                    "-L" => PaneResizeDirection::Left,
                    "-R" => PaneResizeDirection::Right,
                    "-U" => PaneResizeDirection::Up,
                    "-D" => PaneResizeDirection::Down,
                    _ => unreachable!(),
                };
                let amount = if args.get(i + 1).is_some_and(|value| !value.starts_with('-')) {
                    let value = &args[i + 1];
                    i += 2;
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|value| *value > 0)
                        .ok_or_else(|| {
                            "resize-pane amount must be a positive integer".to_string()
                        })?
                } else {
                    i += 1;
                    1
                };
                directional = Some(PaneResize::Directional { direction, amount });
            }
            value => return Err(format!("resize-pane does not support argument {value:?}")),
        }
    }

    let session = session.ok_or_else(|| "resize-pane requires -t <session>".to_string())?;
    if directional.is_some() && (cols.is_some() || rows.is_some()) {
        return Err("resize-pane accepts either -x/-y or one of -L/-R/-U/-D".to_string());
    }
    let resize = if let Some(resize) = directional {
        resize
    } else {
        PaneResize::Absolute {
            cols: cols.ok_or_else(|| "resize-pane requires -x <cols>".to_string())?,
            rows: rows.ok_or_else(|| "resize-pane requires -y <rows>".to_string())?,
        }
    };

    Ok(Command::ResizePane { session, resize })
}

fn parse_send_keys(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut keys = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "send-keys requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            value if value.starts_with('-') => {
                return Err(format!("send-keys does not support option {value:?}"));
            }
            _ => {
                keys.extend(args[i..].iter().cloned());
                break;
            }
        }
    }

    if keys.is_empty() {
        return Err("send-keys requires at least one key".to_string());
    }

    Ok(Command::SendKeys {
        session: session.ok_or_else(|| "send-keys requires -t <session>".to_string())?,
        keys,
    })
}

fn parse_split_window(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut direction = None;
    let mut command = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "split-window requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-h" => {
                set_split_direction(&mut direction, SplitDirection::Horizontal)?;
                i += 1;
            }
            "-v" => {
                set_split_direction(&mut direction, SplitDirection::Vertical)?;
                i += 1;
            }
            "--" => {
                command.extend(args[i + 1..].iter().cloned());
                break;
            }
            value if value.starts_with('-') => {
                return Err(format!("split-window does not support option {value:?}"));
            }
            _ => {
                command.extend(args[i..].iter().cloned());
                break;
            }
        }
    }

    Ok(Command::SplitWindow {
        session: session.ok_or_else(|| "split-window requires -t <session>".to_string())?,
        direction: direction.ok_or_else(|| "split-window requires one of -h or -v".to_string())?,
        command,
    })
}

fn parse_list_panes(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut format = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "list-panes requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-F" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "list-panes requires a format after -F".to_string())?;
                format = Some(value.clone());
                i += 2;
            }
            value => return Err(format!("list-panes does not support argument {value:?}")),
        }
    }

    Ok(Command::ListPanes {
        session: session.ok_or_else(|| "list-panes requires -t <session>".to_string())?,
        format,
    })
}

fn parse_select_pane(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut target = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "select-pane requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-p" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "select-pane requires a pane index after -p".to_string())?;
                set_pane_select_target(
                    &mut target,
                    PaneSelectTarget::Index(value.parse::<usize>().map_err(|_| {
                        "select-pane -p must be a non-negative integer".to_string()
                    })?),
                )?;
                i += 2;
            }
            "--pane-id" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "select-pane requires a pane id after --pane-id".to_string())?;
                set_pane_select_target(
                    &mut target,
                    PaneSelectTarget::Id(value.parse::<usize>().map_err(|_| {
                        "select-pane --pane-id must be a non-negative integer".to_string()
                    })?),
                )?;
                i += 2;
            }
            "-L" | "-R" | "-U" | "-D" => {
                let direction = match args[i].as_str() {
                    "-L" => PaneDirection::Left,
                    "-R" => PaneDirection::Right,
                    "-U" => PaneDirection::Up,
                    "-D" => PaneDirection::Down,
                    _ => unreachable!(),
                };
                set_pane_select_target(&mut target, PaneSelectTarget::Direction(direction))?;
                i += 1;
            }
            value => return Err(format!("select-pane does not support argument {value:?}")),
        }
    }

    Ok(Command::SelectPane {
        session: session.ok_or_else(|| "select-pane requires -t <session>".to_string())?,
        target: target.ok_or_else(|| {
            "select-pane requires one of -p <index>, --pane-id <id>, or -L/-R/-U/-D".to_string()
        })?,
    })
}

fn set_pane_select_target(
    target: &mut Option<PaneSelectTarget>,
    value: PaneSelectTarget,
) -> Result<(), String> {
    if target.replace(value).is_some() {
        Err("select-pane accepts only one pane target".to_string())
    } else {
        Ok(())
    }
}

fn parse_kill_pane(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut pane = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "kill-pane requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-p" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "kill-pane requires a pane index after -p".to_string())?;
                pane = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| "kill-pane -p must be a non-negative integer".to_string())?,
                );
                i += 2;
            }
            value => return Err(format!("kill-pane does not support argument {value:?}")),
        }
    }

    Ok(Command::KillPane {
        session: session.ok_or_else(|| "kill-pane requires -t <session>".to_string())?,
        pane,
    })
}

fn parse_respawn_pane(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut pane = None;
    let mut force = false;
    let mut command = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "respawn-pane requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-p" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "respawn-pane requires a pane index after -p".to_string())?;
                pane =
                    Some(value.parse::<usize>().map_err(|_| {
                        "respawn-pane -p must be a non-negative integer".to_string()
                    })?);
                i += 2;
            }
            "-k" => {
                force = true;
                i += 1;
            }
            "--" => {
                command = args[i + 1..].to_vec();
                break;
            }
            value => return Err(format!("respawn-pane does not support argument {value:?}")),
        }
    }

    Ok(Command::RespawnPane {
        session: session.ok_or_else(|| "respawn-pane requires -t <session>".to_string())?,
        pane,
        force,
        command,
    })
}

fn parse_new_window(args: Vec<String>, command_name: &str) -> Result<Command, String> {
    let mut session = None;
    let mut command = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a session name after -t"))?;
                session = Some(value.clone());
                i += 2;
            }
            "--" => {
                command.extend(args[i + 1..].iter().cloned());
                break;
            }
            value if value.starts_with('-') => {
                return Err(format!("{command_name} does not support option {value:?}"));
            }
            _ => {
                command.extend(args[i..].iter().cloned());
                break;
            }
        }
    }

    Ok(Command::NewWindow {
        session: session.ok_or_else(|| format!("{command_name} requires -t <session>"))?,
        command,
    })
}

fn parse_list_windows(args: Vec<String>, command_name: &str) -> Result<Command, String> {
    let mut session = None;
    let mut format = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a session name after -t"))?;
                session = Some(value.clone());
                i += 2;
            }
            "-F" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a format after -F"))?;
                format = Some(value.clone());
                i += 2;
            }
            value => {
                return Err(format!(
                    "{command_name} does not support argument {value:?}"
                ));
            }
        }
    }

    Ok(Command::ListWindows {
        session: session.ok_or_else(|| format!("{command_name} requires -t <session>"))?,
        format,
    })
}

fn parse_select_window(
    args: Vec<String>,
    command_name: &str,
    primary_index_flag: &str,
    index_flags: &[&str],
    id_flag: &str,
) -> Result<Command, String> {
    let mut session = None;
    let mut target = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a session name after -t"))?;
                session = Some(value.clone());
                i += 2;
            }
            value if index_flags.contains(&value) => {
                let value = args.get(i + 1).ok_or_else(|| {
                    format!(
                        "{command_name} requires a tab/window index after {}",
                        args[i]
                    )
                })?;
                set_window_target(
                    &mut target,
                    WindowTarget::Index(value.parse::<usize>().map_err(|_| {
                        format!("{command_name} {} must be a non-negative integer", args[i])
                    })?),
                    command_name,
                )?;
                i += 2;
            }
            value if value == id_flag => {
                let value = args.get(i + 1).ok_or_else(|| {
                    format!("{command_name} requires a window id after {id_flag}")
                })?;
                set_window_target(
                    &mut target,
                    WindowTarget::Id(value.parse::<usize>().map_err(|_| {
                        format!("{command_name} {id_flag} must be a non-negative integer")
                    })?),
                    command_name,
                )?;
                i += 2;
            }
            "-n" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a window name after -n"))?;
                set_window_target(&mut target, WindowTarget::Name(value.clone()), command_name)?;
                i += 2;
            }
            value => {
                return Err(format!(
                    "{command_name} does not support argument {value:?}"
                ));
            }
        }
    }

    Ok(Command::SelectWindow {
        session: session.ok_or_else(|| format!("{command_name} requires -t <session>"))?,
        target: target.ok_or_else(|| {
            format!(
                "{command_name} requires {primary_index_flag} <index>, {id_flag} <id>, or -n <name>"
            )
        })?,
    })
}

fn parse_rename_window(
    args: Vec<String>,
    command_name: &str,
    index_flags: &[&str],
    id_flag: &str,
) -> Result<Command, String> {
    let mut session = None;
    let mut target = None;
    let mut new_name = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a session name after -t"))?;
                session = Some(value.clone());
                i += 2;
            }
            value if index_flags.contains(&value) => {
                let value = args.get(i + 1).ok_or_else(|| {
                    format!(
                        "{command_name} requires a tab/window index after {}",
                        args[i]
                    )
                })?;
                set_window_target(
                    &mut target,
                    WindowTarget::Index(value.parse::<usize>().map_err(|_| {
                        format!("{command_name} {} must be a non-negative integer", args[i])
                    })?),
                    command_name,
                )?;
                i += 2;
            }
            value if value == id_flag => {
                let value = args.get(i + 1).ok_or_else(|| {
                    format!("{command_name} requires a window id after {id_flag}")
                })?;
                set_window_target(
                    &mut target,
                    WindowTarget::Id(value.parse::<usize>().map_err(|_| {
                        format!("{command_name} {id_flag} must be a non-negative integer")
                    })?),
                    command_name,
                )?;
                i += 2;
            }
            "-n" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a window name after -n"))?;
                set_window_target(&mut target, WindowTarget::Name(value.clone()), command_name)?;
                i += 2;
            }
            value if value.starts_with('-') => {
                return Err(format!("{command_name} does not support option {value:?}"));
            }
            value => {
                if new_name.replace(value.to_string()).is_some() {
                    return Err(format!("{command_name} accepts exactly one new name"));
                }
                i += 1;
            }
        }
    }

    let name = new_name.ok_or_else(|| format!("{command_name} requires <new-name>"))?;
    if name.is_empty() {
        return Err(format!("{command_name} new name cannot be empty"));
    }

    Ok(Command::RenameWindow {
        session: session.ok_or_else(|| format!("{command_name} requires -t <session>"))?,
        target: target.unwrap_or(WindowTarget::Active),
        name,
    })
}

fn parse_cycle_window(
    args: Vec<String>,
    command_name: &str,
    next: bool,
) -> Result<Command, String> {
    let session = parse_target(args, command_name)?
        .ok_or_else(|| format!("{command_name} requires -t <session>"))?;
    if next {
        Ok(Command::NextWindow { session })
    } else {
        Ok(Command::PreviousWindow { session })
    }
}

fn set_window_target(
    target: &mut Option<WindowTarget>,
    value: WindowTarget,
    command_name: &str,
) -> Result<(), String> {
    if target.replace(value).is_some() {
        Err(format!("{command_name} accepts exactly one window target"))
    } else {
        Ok(())
    }
}

fn parse_kill_window(
    args: Vec<String>,
    command_name: &str,
    index_flags: &[&str],
) -> Result<Command, String> {
    let mut session = None;
    let mut window = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a session name after -t"))?;
                session = Some(value.clone());
                i += 2;
            }
            value if index_flags.contains(&value) => {
                let value = args.get(i + 1).ok_or_else(|| {
                    format!(
                        "{command_name} requires a tab/window index after {}",
                        args[i]
                    )
                })?;
                window = Some(value.parse::<usize>().map_err(|_| {
                    format!("{command_name} {} must be a non-negative integer", args[i])
                })?);
                i += 2;
            }
            value => {
                return Err(format!(
                    "{command_name} does not support argument {value:?}"
                ));
            }
        }
    }

    Ok(Command::KillWindow {
        session: session.ok_or_else(|| format!("{command_name} requires -t <session>"))?,
        window,
    })
}

fn parse_zoom_pane(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut pane = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "zoom-pane requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-p" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "zoom-pane requires a pane index after -p".to_string())?;
                pane = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| "zoom-pane -p must be a non-negative integer".to_string())?,
                );
                i += 2;
            }
            value => return Err(format!("zoom-pane does not support argument {value:?}")),
        }
    }

    Ok(Command::ZoomPane {
        session: session.ok_or_else(|| "zoom-pane requires -t <session>".to_string())?,
        pane,
    })
}

fn parse_status_line(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut format = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "status-line requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-F" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "status-line requires a format after -F".to_string())?;
                format = Some(value.clone());
                i += 2;
            }
            value => return Err(format!("status-line does not support argument {value:?}")),
        }
    }

    Ok(Command::StatusLine {
        session: session.ok_or_else(|| "status-line requires -t <session>".to_string())?,
        format,
    })
}

fn parse_display_message(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut format = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "display-message requires a session name after -t".to_string()
                })?;
                session = Some(value.clone());
                i += 2;
            }
            "-p" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "display-message requires a format after -p".to_string())?;
                format = Some(value.clone());
                i += 2;
            }
            value => {
                return Err(format!(
                    "display-message does not support argument {value:?}"
                ));
            }
        }
    }

    Ok(Command::DisplayMessage {
        session: session.ok_or_else(|| "display-message requires -t <session>".to_string())?,
        format: format.ok_or_else(|| "display-message requires -p <format>".to_string())?,
    })
}

fn set_split_direction(
    direction: &mut Option<SplitDirection>,
    value: SplitDirection,
) -> Result<(), String> {
    if direction.replace(value).is_some() {
        Err("split-window accepts only one direction".to_string())
    } else {
        Ok(())
    }
}

fn parse_target(args: Vec<String>, command: &str) -> Result<Option<String>, String> {
    let mut target = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command} requires a session name after -t"))?;
                target = Some(value.clone());
                i += 2;
            }
            "-p" if command == "capture-pane" => {
                i += 1;
            }
            value => return Err(format!("{command} does not support argument {value:?}")),
        }
    }

    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_detached_new_session_with_command() {
        let command = parse_args([
            "dmux", "new", "-d", "-s", "dev", "--", "sh", "-c", "echo ok",
        ])
        .unwrap();

        assert_eq!(
            command,
            Command::New {
                session: "dev".to_string(),
                detach: true,
                command: vec!["sh".to_string(), "-c".to_string(), "echo ok".to_string()],
            }
        );
    }

    #[test]
    fn parses_top_level_help() {
        assert_eq!(
            parse_args(["dmux", "--help"]).unwrap(),
            Command::Help { topic: None }
        );
        assert_eq!(
            parse_args(["dmux", "help"]).unwrap(),
            Command::Help { topic: None }
        );
    }

    #[test]
    fn parses_no_subcommand_as_open_default() {
        assert_eq!(parse_args(["dmux"]).unwrap(), Command::OpenDefault);
    }

    #[test]
    fn parses_session_lifecycle_commands() {
        assert_eq!(
            parse_args(["dmux", "list-sessions", "-F", "#{session.name}"]).unwrap(),
            Command::ListSessions {
                format: Some("#{session.name}".to_string()),
            }
        );
        assert_eq!(
            parse_args(["dmux", "rename-session", "-t", "old", "new"]).unwrap(),
            Command::RenameSession {
                old_name: "old".to_string(),
                new_name: "new".to_string(),
            }
        );
        assert_eq!(
            parse_args(["dmux", "list-clients", "-t", "dev", "-F", "#{client.id}"]).unwrap(),
            Command::ListClients {
                session: Some("dev".to_string()),
                format: Some("#{client.id}".to_string()),
            }
        );
        assert_eq!(
            parse_args(["dmux", "detach-client", "-t", "dev", "-c", "3"]).unwrap(),
            Command::DetachClient {
                session: Some("dev".to_string()),
                client_id: Some(3),
            }
        );
    }

    #[test]
    fn rejects_detach_client_without_target() {
        let err = parse_args(["dmux", "detach-client"]).unwrap_err();
        assert!(err.contains("requires -t"), "{err}");
    }

    #[test]
    fn parses_attach_help() {
        assert_eq!(
            parse_args(["dmux", "attach", "--help"]).unwrap(),
            Command::Help {
                topic: Some(HelpTopic::Attach),
            }
        );
        assert_eq!(
            parse_args(["dmux", "help", "attach"]).unwrap(),
            Command::Help {
                topic: Some(HelpTopic::Attach),
            }
        );
    }

    #[test]
    fn parses_attach_target_named_help_literal() {
        assert_eq!(
            parse_args(["dmux", "attach", "-t", "--help"]).unwrap(),
            Command::Attach {
                session: "--help".to_string(),
            }
        );
    }

    #[test]
    fn attach_help_lists_prefix_bindings_and_split_command() {
        let help = attach_help();

        assert!(help.contains("C-b d"), "{help}");
        assert!(help.contains("Usage: dmux attach [-t <name>]"), "{help}");
        assert!(help.contains("omitted"), "{help}");
        assert!(help.contains("C-b %"), "{help}");
        assert!(help.contains("C-b \""), "{help}");
        assert!(help.contains("C-b h/j/k/l"), "{help}");
        assert!(help.contains("C-b x"), "{help}");
        assert!(help.contains("C-b z"), "{help}");
        assert!(help.contains("C-b ?"), "{help}");
        assert!(help.contains("C-b o"), "{help}");
        assert!(help.contains("split-window"), "{help}");
    }

    #[test]
    fn parses_capture_pane_print() {
        let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p"]).unwrap();
        assert_eq!(
            command,
            Command::CapturePane {
                session: "dev".to_string(),
                mode: CaptureMode::All,
                selection: BufferSelection::All,
            }
        );
    }

    #[test]
    fn parses_capture_pane_screen_mode() {
        let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p", "--screen"]).unwrap();
        assert_eq!(
            command,
            Command::CapturePane {
                session: "dev".to_string(),
                mode: CaptureMode::Screen,
                selection: BufferSelection::All,
            }
        );
    }

    #[test]
    fn parses_capture_pane_history_mode() {
        let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p", "--history"]).unwrap();
        assert_eq!(
            command,
            Command::CapturePane {
                session: "dev".to_string(),
                mode: CaptureMode::History,
                selection: BufferSelection::All,
            }
        );
    }

    #[test]
    fn parses_capture_pane_all_mode() {
        let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p", "--all"]).unwrap();
        assert_eq!(
            command,
            Command::CapturePane {
                session: "dev".to_string(),
                mode: CaptureMode::All,
                selection: BufferSelection::All,
            }
        );
    }

    #[test]
    fn parses_capture_pane_tail_line_range() {
        let command = parse_args([
            "dmux",
            "capture-pane",
            "-t",
            "dev",
            "-p",
            "--start-line",
            "-2",
            "--end-line",
            "-1",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::CapturePane {
                session: "dev".to_string(),
                mode: CaptureMode::All,
                selection: BufferSelection::LineRange { start: -2, end: -1 },
            }
        );
    }

    #[test]
    fn parses_capture_pane_mixed_positive_tail_line_range() {
        let command = parse_args([
            "dmux",
            "capture-pane",
            "-t",
            "dev",
            "-p",
            "--start-line",
            "2",
            "--end-line",
            "-1",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::CapturePane {
                session: "dev".to_string(),
                mode: CaptureMode::All,
                selection: BufferSelection::LineRange { start: 2, end: -1 },
            }
        );
    }

    #[test]
    fn parses_capture_pane_search_match_selection() {
        let command = parse_args([
            "dmux",
            "capture-pane",
            "-t",
            "dev",
            "-p",
            "--screen",
            "--search",
            "needle",
            "--match",
            "2",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::CapturePane {
                session: "dev".to_string(),
                mode: CaptureMode::Screen,
                selection: BufferSelection::Search {
                    needle: "needle".to_string(),
                    match_index: 2,
                },
            }
        );
    }

    #[test]
    fn rejects_capture_pane_search_with_line_range() {
        let err = parse_args([
            "dmux",
            "capture-pane",
            "-t",
            "dev",
            "--start-line",
            "1",
            "--end-line",
            "2",
            "--search",
            "needle",
        ])
        .unwrap_err();
        assert!(err.contains("either a line range or --search"), "{err}");
    }

    #[test]
    fn rejects_capture_pane_match_without_search() {
        let err = parse_args(["dmux", "capture-pane", "-t", "dev", "--match", "2"]).unwrap_err();
        assert!(err.contains("--match requires --search"), "{err}");
    }

    #[test]
    fn rejects_multiple_capture_pane_modes() {
        let err = parse_args([
            "dmux",
            "capture-pane",
            "-t",
            "dev",
            "-p",
            "--screen",
            "--history",
        ])
        .unwrap_err();
        assert!(err.contains("only one capture mode"), "{err}");
    }

    #[test]
    fn parses_save_buffer_named_screen_capture() {
        let command = parse_args([
            "dmux",
            "save-buffer",
            "-t",
            "dev",
            "-b",
            "saved",
            "--screen",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::SaveBuffer {
                session: "dev".to_string(),
                buffer: Some("saved".to_string()),
                mode: CaptureMode::Screen,
                selection: BufferSelection::All,
            }
        );
    }

    #[test]
    fn parses_save_buffer_line_range_selection() {
        let command = parse_args([
            "dmux",
            "save-buffer",
            "-t",
            "dev",
            "-b",
            "picked",
            "--screen",
            "--start-line",
            "2",
            "--end-line",
            "3",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::SaveBuffer {
                session: "dev".to_string(),
                buffer: Some("picked".to_string()),
                mode: CaptureMode::Screen,
                selection: BufferSelection::LineRange { start: 2, end: 3 },
            }
        );
    }

    #[test]
    fn parses_save_buffer_mixed_positive_tail_line_range() {
        let command = parse_args([
            "dmux",
            "save-buffer",
            "-t",
            "dev",
            "-b",
            "picked",
            "--screen",
            "--start-line",
            "2",
            "--end-line",
            "-1",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::SaveBuffer {
                session: "dev".to_string(),
                buffer: Some("picked".to_string()),
                mode: CaptureMode::Screen,
                selection: BufferSelection::LineRange { start: 2, end: -1 },
            }
        );
    }

    #[test]
    fn parses_save_buffer_search_selection() {
        let command = parse_args([
            "dmux",
            "save-buffer",
            "-t",
            "dev",
            "-b",
            "match",
            "--search",
            "needle",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::SaveBuffer {
                session: "dev".to_string(),
                buffer: Some("match".to_string()),
                mode: CaptureMode::All,
                selection: BufferSelection::Search {
                    needle: "needle".to_string(),
                    match_index: 1,
                },
            }
        );
    }

    #[test]
    fn rejects_save_buffer_search_with_line_range() {
        let err = parse_args([
            "dmux",
            "save-buffer",
            "-t",
            "dev",
            "--start-line",
            "1",
            "--end-line",
            "2",
            "--search",
            "needle",
        ])
        .unwrap_err();
        assert!(err.contains("either a line range or --search"), "{err}");
    }

    #[test]
    fn rejects_save_buffer_partial_line_range() {
        let err =
            parse_args(["dmux", "save-buffer", "-t", "dev", "--start-line", "1"]).unwrap_err();
        assert!(err.contains("--start-line and --end-line"), "{err}");
    }

    #[test]
    fn parses_copy_mode_history_search() {
        let command = parse_args([
            "dmux",
            "copy-mode",
            "-t",
            "dev",
            "--history",
            "--search",
            "needle",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::CopyMode {
                session: "dev".to_string(),
                mode: CaptureMode::History,
                search: Some("needle".to_string()),
                match_index: None,
            }
        );
    }

    #[test]
    fn parses_copy_mode_search_match_index() {
        let command = parse_args([
            "dmux",
            "copy-mode",
            "-t",
            "dev",
            "--search",
            "needle",
            "--match",
            "2",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::CopyMode {
                session: "dev".to_string(),
                mode: CaptureMode::All,
                search: Some("needle".to_string()),
                match_index: Some(2),
            }
        );
    }

    #[test]
    fn parses_copy_mode_default_all() {
        assert_eq!(
            parse_args(["dmux", "copy-mode", "-t", "dev"]).unwrap(),
            Command::CopyMode {
                session: "dev".to_string(),
                mode: CaptureMode::All,
                search: None,
                match_index: None,
            }
        );
    }

    #[test]
    fn rejects_copy_mode_multiple_capture_modes() {
        let err =
            parse_args(["dmux", "copy-mode", "-t", "dev", "--screen", "--history"]).unwrap_err();
        assert!(err.contains("only one capture mode"), "{err}");
    }

    #[test]
    fn rejects_copy_mode_empty_search() {
        let err = parse_args(["dmux", "copy-mode", "-t", "dev", "--search", ""]).unwrap_err();
        assert!(err.contains("non-empty text"), "{err}");
    }

    #[test]
    fn parses_list_buffers() {
        assert_eq!(
            parse_args(["dmux", "list-buffers"]).unwrap(),
            Command::ListBuffers { format: None }
        );
    }

    #[test]
    fn parses_list_buffers_format() {
        assert_eq!(
            parse_args(["dmux", "list-buffers", "-F", "#{buffer.name}"]).unwrap(),
            Command::ListBuffers {
                format: Some("#{buffer.name}".to_string())
            }
        );
    }

    #[test]
    fn parses_paste_buffer_named_target() {
        let command = parse_args(["dmux", "paste-buffer", "-t", "dev", "-b", "saved"]).unwrap();
        assert_eq!(
            command,
            Command::PasteBuffer {
                session: "dev".to_string(),
                buffer: Some("saved".to_string()),
            }
        );
    }

    #[test]
    fn parses_delete_buffer_named() {
        assert_eq!(
            parse_args(["dmux", "delete-buffer", "-b", "saved"]).unwrap(),
            Command::DeleteBuffer {
                buffer: "saved".to_string(),
            }
        );
    }

    #[test]
    fn rejects_empty_buffer_name() {
        let err = parse_args(["dmux", "save-buffer", "-t", "dev", "-b", ""]).unwrap_err();
        assert!(err.contains("non-empty buffer name"), "{err}");
    }

    #[test]
    fn rejects_missing_session_target() {
        let err = parse_args(["dmux", "kill-session"]).unwrap_err();
        assert!(err.contains("-t"));
    }

    #[test]
    fn parses_resize_pane_target_and_size() {
        let command =
            parse_args(["dmux", "resize-pane", "-t", "dev", "-x", "100", "-y", "40"]).unwrap();
        assert_eq!(
            command,
            Command::ResizePane {
                session: "dev".to_string(),
                resize: PaneResize::Absolute {
                    cols: 100,
                    rows: 40
                },
            }
        );
    }

    #[test]
    fn parses_resize_pane_direction_default_amount() {
        let command = parse_args(["dmux", "resize-pane", "-t", "dev", "-L"]).unwrap();
        assert_eq!(
            command,
            Command::ResizePane {
                session: "dev".to_string(),
                resize: PaneResize::Directional {
                    direction: PaneResizeDirection::Left,
                    amount: 1,
                },
            }
        );
    }

    #[test]
    fn parses_resize_pane_direction_amount() {
        let command = parse_args(["dmux", "resize-pane", "-t", "dev", "-D", "5"]).unwrap();
        assert_eq!(
            command,
            Command::ResizePane {
                session: "dev".to_string(),
                resize: PaneResize::Directional {
                    direction: PaneResizeDirection::Down,
                    amount: 5,
                },
            }
        );
    }

    #[test]
    fn rejects_resize_pane_zero_direction_amount() {
        let err = parse_args(["dmux", "resize-pane", "-t", "dev", "-R", "0"]).unwrap_err();
        assert!(err.contains("positive integer"), "{err}");
    }

    #[test]
    fn rejects_resize_pane_mixed_absolute_and_directional() {
        let err = parse_args([
            "dmux",
            "resize-pane",
            "-t",
            "dev",
            "-x",
            "80",
            "-y",
            "24",
            "-L",
        ])
        .unwrap_err();
        assert!(err.contains("either"), "{err}");
    }

    #[test]
    fn parses_send_keys_target_and_keys() {
        let command = parse_args(["dmux", "send-keys", "-t", "dev", "echo hi", "Enter"]).unwrap();
        assert_eq!(
            command,
            Command::SendKeys {
                session: "dev".to_string(),
                keys: vec!["echo hi".to_string(), "Enter".to_string()],
            }
        );
    }

    #[test]
    fn parses_split_window_target_direction_and_command() {
        let command = parse_args([
            "dmux",
            "split-window",
            "-t",
            "dev",
            "-h",
            "--",
            "sh",
            "-c",
            "echo split",
        ])
        .unwrap();

        assert_eq!(
            command,
            Command::SplitWindow {
                session: "dev".to_string(),
                direction: crate::protocol::SplitDirection::Horizontal,
                command: vec!["sh".to_string(), "-c".to_string(), "echo split".to_string()],
            }
        );
    }

    #[test]
    fn parses_list_panes_target() {
        let command = parse_args(["dmux", "list-panes", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::ListPanes {
                session: "dev".to_string(),
                format: None,
            }
        );
    }

    #[test]
    fn parses_list_panes_format() {
        let command = parse_args([
            "dmux",
            "list-panes",
            "-t",
            "dev",
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::ListPanes {
                session: "dev".to_string(),
                format: Some("#{pane.index}:#{pane.active}:#{pane.zoomed}".to_string()),
            }
        );
    }

    #[test]
    fn parses_select_pane_target_and_index() {
        let command = parse_args(["dmux", "select-pane", "-t", "dev", "-p", "1"]).unwrap();
        assert_eq!(
            command,
            Command::SelectPane {
                session: "dev".to_string(),
                target: PaneSelectTarget::Index(1),
            }
        );
    }

    #[test]
    fn parses_select_pane_target_and_pane_id() {
        let command = parse_args(["dmux", "select-pane", "-t", "dev", "--pane-id", "42"]).unwrap();
        assert_eq!(
            command,
            Command::SelectPane {
                session: "dev".to_string(),
                target: PaneSelectTarget::Id(42),
            }
        );
    }

    #[test]
    fn parses_select_pane_direction() {
        let command = parse_args(["dmux", "select-pane", "-t", "dev", "-L"]).unwrap();
        assert_eq!(
            command,
            Command::SelectPane {
                session: "dev".to_string(),
                target: PaneSelectTarget::Direction(PaneDirection::Left),
            }
        );
    }

    #[test]
    fn rejects_select_pane_conflicting_targets() {
        let err = parse_args([
            "dmux",
            "select-pane",
            "-t",
            "dev",
            "-p",
            "1",
            "--pane-id",
            "42",
        ])
        .unwrap_err();
        assert!(err.contains("only one pane target"), "{err}");

        let err = parse_args(["dmux", "select-pane", "-t", "dev", "-p", "1", "-L"]).unwrap_err();
        assert!(err.contains("only one pane target"), "{err}");
    }

    #[test]
    fn rejects_select_pane_without_target() {
        let err = parse_args(["dmux", "select-pane", "-t", "dev"]).unwrap_err();
        assert!(err.contains("requires one of"), "{err}");
    }

    #[test]
    fn parses_kill_pane_target_and_index() {
        let command = parse_args(["dmux", "kill-pane", "-t", "dev", "-p", "1"]).unwrap();
        assert_eq!(
            command,
            Command::KillPane {
                session: "dev".to_string(),
                pane: Some(1),
            }
        );
    }

    #[test]
    fn parses_kill_pane_target_without_index() {
        let command = parse_args(["dmux", "kill-pane", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::KillPane {
                session: "dev".to_string(),
                pane: None,
            }
        );
    }

    #[test]
    fn parses_respawn_pane_target_force_and_command() {
        let command = parse_args([
            "dmux",
            "respawn-pane",
            "-t",
            "dev",
            "-p",
            "1",
            "-k",
            "--",
            "sh",
            "-c",
            "echo ready",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::RespawnPane {
                session: "dev".to_string(),
                pane: Some(1),
                force: true,
                command: vec!["sh".to_string(), "-c".to_string(), "echo ready".to_string()],
            }
        );
    }

    #[test]
    fn parses_new_window_target_and_command() {
        let command = parse_args([
            "dmux",
            "new-window",
            "-t",
            "dev",
            "--",
            "sh",
            "-c",
            "echo window",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::NewWindow {
                session: "dev".to_string(),
                command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "echo window".to_string()
                ],
            }
        );
    }

    #[test]
    fn parses_new_tab_alias_target_and_command() {
        let command = parse_args(["dmux", "new-tab", "-t", "dev", "--", "echo", "tab"]).unwrap();
        assert_eq!(
            command,
            Command::NewWindow {
                session: "dev".to_string(),
                command: vec!["echo".to_string(), "tab".to_string()],
            }
        );
    }

    #[test]
    fn parses_list_windows_target() {
        let command = parse_args(["dmux", "list-windows", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::ListWindows {
                session: "dev".to_string(),
                format: None,
            }
        );
    }

    #[test]
    fn parses_list_tabs_alias_target() {
        let command = parse_args(["dmux", "list-tabs", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::ListWindows {
                session: "dev".to_string(),
                format: None,
            }
        );
    }

    #[test]
    fn parses_select_window_target_and_index() {
        let command = parse_args(["dmux", "select-window", "-t", "dev", "-w", "1"]).unwrap();
        assert_eq!(
            command,
            Command::SelectWindow {
                session: "dev".to_string(),
                target: WindowTarget::Index(1),
            }
        );
    }

    #[test]
    fn parses_select_tab_alias_target_and_index() {
        let command = parse_args(["dmux", "select-tab", "-t", "dev", "-i", "1"]).unwrap();
        assert_eq!(
            command,
            Command::SelectWindow {
                session: "dev".to_string(),
                target: WindowTarget::Index(1),
            }
        );
    }

    #[test]
    fn parses_select_window_by_id_and_name() {
        assert_eq!(
            parse_args(["dmux", "select-window", "-t", "dev", "--window-id", "42"]).unwrap(),
            Command::SelectWindow {
                session: "dev".to_string(),
                target: WindowTarget::Id(42),
            }
        );
        assert_eq!(
            parse_args(["dmux", "select-tab", "-t", "dev", "-n", "editor"]).unwrap(),
            Command::SelectWindow {
                session: "dev".to_string(),
                target: WindowTarget::Name("editor".to_string()),
            }
        );
    }

    #[test]
    fn rejects_conflicting_window_targets() {
        let err = parse_args([
            "dmux",
            "select-window",
            "-t",
            "dev",
            "-w",
            "1",
            "-n",
            "editor",
        ])
        .unwrap_err();
        assert_eq!(err, "select-window accepts exactly one window target");
    }

    #[test]
    fn parses_rename_and_cycle_window_commands() {
        assert_eq!(
            parse_args(["dmux", "rename-window", "-t", "dev", "editor"]).unwrap(),
            Command::RenameWindow {
                session: "dev".to_string(),
                target: WindowTarget::Active,
                name: "editor".to_string(),
            }
        );
        assert_eq!(
            parse_args(["dmux", "rename-tab", "-t", "dev", "--tab-id", "7", "logs"]).unwrap(),
            Command::RenameWindow {
                session: "dev".to_string(),
                target: WindowTarget::Id(7),
                name: "logs".to_string(),
            }
        );
        assert_eq!(
            parse_args(["dmux", "next-window", "-t", "dev"]).unwrap(),
            Command::NextWindow {
                session: "dev".to_string(),
            }
        );
        assert_eq!(
            parse_args(["dmux", "previous-tab", "-t", "dev"]).unwrap(),
            Command::PreviousWindow {
                session: "dev".to_string(),
            }
        );
    }

    #[test]
    fn rejects_empty_rename_and_conflicting_rename_targets() {
        assert_eq!(
            parse_args(["dmux", "rename-window", "-t", "dev", ""]).unwrap_err(),
            "rename-window new name cannot be empty"
        );
        assert_eq!(
            parse_args([
                "dmux",
                "rename-tab",
                "-t",
                "dev",
                "-i",
                "0",
                "--tab-id",
                "1",
                "name"
            ])
            .unwrap_err(),
            "rename-tab accepts exactly one window target"
        );
    }

    #[test]
    fn parses_kill_window_target_and_index() {
        let command = parse_args(["dmux", "kill-window", "-t", "dev", "-w", "1"]).unwrap();
        assert_eq!(
            command,
            Command::KillWindow {
                session: "dev".to_string(),
                window: Some(1),
            }
        );
    }

    #[test]
    fn parses_kill_tab_alias_target_and_index() {
        let command = parse_args(["dmux", "kill-tab", "-t", "dev", "-i", "1"]).unwrap();
        assert_eq!(
            command,
            Command::KillWindow {
                session: "dev".to_string(),
                window: Some(1),
            }
        );
    }

    #[test]
    fn parses_kill_window_target_without_index() {
        let command = parse_args(["dmux", "kill-window", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::KillWindow {
                session: "dev".to_string(),
                window: None,
            }
        );
    }

    #[test]
    fn parses_zoom_pane_target_without_index() {
        let command = parse_args(["dmux", "zoom-pane", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::ZoomPane {
                session: "dev".to_string(),
                pane: None,
            }
        );
    }

    #[test]
    fn parses_zoom_pane_target_and_index() {
        let command = parse_args(["dmux", "zoom-pane", "-t", "dev", "-p", "0"]).unwrap();
        assert_eq!(
            command,
            Command::ZoomPane {
                session: "dev".to_string(),
                pane: Some(0),
            }
        );
    }

    #[test]
    fn parses_status_line_target() {
        let command = parse_args(["dmux", "status-line", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::StatusLine {
                session: "dev".to_string(),
                format: None,
            }
        );
    }

    #[test]
    fn parses_status_line_format() {
        let command = parse_args([
            "dmux",
            "status-line",
            "-t",
            "dev",
            "-F",
            "#{session.name}:#{window.index}:#{pane.index}",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::StatusLine {
                session: "dev".to_string(),
                format: Some("#{session.name}:#{window.index}:#{pane.index}".to_string()),
            }
        );
    }

    #[test]
    fn parses_display_message_print_format() {
        let command = parse_args([
            "dmux",
            "display-message",
            "-t",
            "dev",
            "-p",
            "#{session.name}:#{pane.index}",
        ])
        .unwrap();
        assert_eq!(
            command,
            Command::DisplayMessage {
                session: "dev".to_string(),
                format: "#{session.name}:#{pane.index}".to_string(),
            }
        );
    }
}
