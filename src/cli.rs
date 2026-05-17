use crate::protocol::{
    BufferSelection, CaptureMode, LayoutPreset, PaneDirection, PaneResizeDirection,
    PaneSelectTarget, PaneTarget, SplitDirection, Target, WindowTarget,
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
    ResizePane {
        target: Target,
        resize: PaneResize,
    },
    SelectLayout {
        session: String,
        window: WindowTarget,
        preset: LayoutPreset,
    },
    SendKeys {
        target: Target,
        keys: Vec<String>,
    },
    SplitWindow {
        target: Target,
        direction: SplitDirection,
        command: Vec<String>,
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
    Run {
        sequence: String,
    },
    SourceFile {
        path: String,
    },
    RunShell {
        command: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptCommand {
    pub source: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandFileEntry {
    pub line: usize,
    pub command: ScriptCommand,
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
        "select-layout" => parse_select_layout(args),
        "send-keys" => parse_send_keys(args),
        "split-window" | "split" => parse_split_window(args),
        "list-panes" => parse_list_panes(args),
        "select-pane" => parse_select_pane(args),
        "kill-pane" => parse_kill_pane(args),
        "swap-pane" => parse_swap_pane(args),
        "move-pane" => parse_move_pane(args),
        "break-pane" => parse_break_pane(args),
        "join-pane" => parse_join_pane(args),
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
        "list-keys" => parse_list_keys(args),
        "bind-key" => parse_bind_key(args),
        "unbind-key" => parse_unbind_key(args),
        "show-options" => parse_show_options(args),
        "set-option" | "set" => parse_set_option(args),
        "run" | "command" => parse_run(args),
        "source-file" => parse_source_file(args),
        "run-shell" => parse_run_shell(args),
        "kill-session" => parse_kill_session(args),
        "kill-server" => Ok(Command::KillServer),
        _ => Err(format!("{program}: unknown command {subcommand:?}")),
    }
}

pub fn parse_command_sequence(input: &str) -> Result<Vec<ScriptCommand>, String> {
    split_command_sequence(input)?
        .into_iter()
        .map(|source| {
            let argv = tokenize_command(&source)?;
            Ok(ScriptCommand { source, argv })
        })
        .collect()
}

pub fn parse_command_file(contents: &str) -> Result<Vec<CommandFileEntry>, String> {
    let mut commands = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let line_commands =
            parse_command_sequence(line).map_err(|err| format!("line {line_number}: {err}"))?;
        commands.extend(line_commands.into_iter().map(|command| CommandFileEntry {
            line: line_number,
            command,
        }));
    }
    Ok(commands)
}

fn split_command_sequence(input: &str) -> Result<Vec<String>, String> {
    let mut commands = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escape = false;

    for ch in input.chars() {
        if escape {
            current.push('\\');
            current.push(ch);
            escape = false;
            continue;
        }

        match (quote, ch) {
            (Some('\''), '\'') => {
                quote = None;
                current.push(ch);
            }
            (Some('\''), _) => current.push(ch),
            (Some(_), '\\') => {
                escape = true;
            }
            (Some(active), quote_ch) if quote_ch == active => {
                quote = None;
                current.push(ch);
            }
            (Some(_), _) => current.push(ch),
            (None, '\\') => {
                escape = true;
            }
            (None, '\'' | '"') => {
                quote = Some(ch);
                current.push(ch);
            }
            (None, ';') => {
                let command = current.trim();
                if !command.is_empty() {
                    commands.push(command.to_string());
                }
                current.clear();
            }
            (None, _) => current.push(ch),
        }
    }

    if escape {
        return Err("trailing backslash in command sequence".to_string());
    }
    if let Some(active) = quote {
        return Err(format!("unterminated {active} quote in command sequence"));
    }

    let command = current.trim();
    if !command.is_empty() {
        commands.push(command.to_string());
    }
    Ok(commands)
}

pub fn tokenize_command(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escape = false;
    let mut token_started = false;

    for ch in input.chars() {
        if escape {
            current.push(ch);
            token_started = true;
            escape = false;
            continue;
        }

        match (quote, ch) {
            (Some('\''), '\'') => {
                quote = None;
                token_started = true;
            }
            (Some('\''), _) => {
                current.push(ch);
                token_started = true;
            }
            (Some(active), quote_ch) if quote_ch == active => {
                quote = None;
                token_started = true;
            }
            (Some(_), '\\') => {
                escape = true;
                token_started = true;
            }
            (Some(_), _) => {
                current.push(ch);
                token_started = true;
            }
            (None, '\\') => {
                escape = true;
                token_started = true;
            }
            (None, '\'' | '"') => {
                quote = Some(ch);
                token_started = true;
            }
            (None, ch) if ch.is_whitespace() => {
                if token_started {
                    args.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            (None, _) => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if escape {
        return Err("trailing backslash in command".to_string());
    }
    if let Some(active) = quote {
        return Err(format!("unterminated {active} quote in command"));
    }
    if token_started {
        args.push(current);
    }
    Ok(args)
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
  select-layout -t <name> <preset>       apply even-horizontal/even-vertical/tiled/main-*\n\
  list-panes -t <name> [-F <format>]\n\
  select-pane -t <name> -p <index>|--pane-id <id>|-L|-R|-U|-D\n\
  swap-pane -s <target> -t <target>      swap two panes in one window\n\
  move-pane -s <target> -t <target> [-h|-v]\n\
  break-pane -t <target>                 move pane into a new window\n\
  join-pane -s <target> -t <target> [-h|-v]\n\
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
  run <command; command...>              run dmux commands in order; stops on first error\n\
  source-file <path>                     run newline-separated dmux commands from a file\n\
  run-shell <shell-command>              run a host shell command and report its status\n\
  list-keys [-F <format>]                list runtime prefix key bindings\n\
  bind-key <key> <action>                bind prefix key to a supported live action\n\
  unbind-key <key>                       remove a prefix binding\n\
  show-options [-F <format>]             list runtime server options\n\
  set-option <name> <value>              set runtime server option\n\
  kill-session -t <name>\n\
  kill-server\n\
\n\
Targets:\n\
  <session>[:<window>[.<pane>]] where numeric values are indexes,\n\
  @<id> selects a window id, %<id> selects a pane id, and =<name> selects a window name\n\
  Session names cannot contain ':' because ':' separates structured targets\n\
\n\
Help:\n\
  dmux help attach\n\
  dmux attach --help\n"
}

pub fn attach_help() -> &'static str {
    "Usage: dmux attach [-t <name>]\n\
\n\
If -t is omitted, attach targets default. The status line shows the active session,\n\
window, pane, and quick hints for prefix, help, command prompt, and copy-mode.\n\
\n\
Session:\n\
  C-b d detach / C-b D detach    C-b C-b send literal prefix    C-b ? show this help\n\
Windows:\n\
  C-b c new window        C-b n/p next/previous window\n\
Panes:\n\
  C-b % split right       C-b \" split down       C-b o next pane\n\
  C-b h/j/k/l focus       C-b H/J/K/L resize by 5\n\
  C-b q pane numbers      C-b x close pane       C-b z zoom pane\n\
Copy:\n\
  C-b [ copy-mode         copy-mode: j/k arrows PgUp/PgDn y/Enter copy q/Esc exit\n\
Prompt:\n\
  C-b : command prompt    Enter run    Esc/C-c cancel    Backspace edit\n\
  Prompt accepts semicolon-separated commands; source-file reads prompt commands.\n\
  Key bindings and options are runtime/server-scoped; use list-keys/show-options to inspect.\n\
Prompt examples:\n\
  :split -h               :split -v               :rename-window api\n\
  :layout tiled           :swap-pane 1            :break-pane\n\
  :join-pane -s 1.0       :select-window 0        :list-windows\n\
CLI equivalents:\n\
  dmux split-window -t <name> -h|-v [-- command...]\n\
  dmux select-layout -t <name> tiled|even-horizontal|even-vertical|main-horizontal|main-vertical\n\
  dmux resize-pane -t <name> -L|-R|-U|-D [amount]\n\
  dmux select-pane -t <name> -p <index>|--pane-id <id>|-L|-R|-U|-D\n"
}

pub fn attach_help_overlay() -> &'static str {
    "Session:\n\
  C-b d detach / C-b D detach    C-b C-b send literal prefix    C-b ? show this help\n\
Windows:\n\
  C-b c new window        C-b n/p next/previous window\n\
Panes:\n\
  C-b % split right       C-b \" split down       C-b o next pane\n\
  C-b h/j/k/l focus       C-b H/J/K/L resize by 5\n\
  C-b q pane numbers      C-b x close pane       C-b z zoom pane\n\
Copy:\n\
  C-b [ copy-mode         copy-mode: j/k arrows PgUp/PgDn y/Enter copy q/Esc exit\n\
Prompt:\n\
  C-b : command prompt    Enter run    Esc/C-c cancel    Backspace edit\n\
  Prompt accepts semicolon-separated commands; source-file reads prompt commands.\n\
  Key bindings and options are runtime/server-scoped; use list-keys/show-options to inspect.\n\
Prompt examples:\n\
  :split -h               :split -v               :rename-window api\n\
  :layout tiled           :swap-pane 1            :break-pane\n\
  :join-pane -s 1.0       :select-window 0        :list-windows\n\
CLI equivalents:\n\
  dmux split-window -t <name> -h|-v [-- command...]\n\
  dmux select-layout -t <name> tiled|even-horizontal|even-vertical|main-horizontal|main-vertical\n\
  dmux resize-pane -t <name> -L|-R|-U|-D [amount]\n\
  dmux select-pane -t <name> -p <index>|--pane-id <id>|-L|-R|-U|-D\n"
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
    let mut target = None;
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
                target = Some(parse_structured_target(value, "capture-pane")?);
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
        target: target.ok_or_else(|| "capture-pane requires -t <session>".to_string())?,
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
    let mut target = None;
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
                target = Some(parse_structured_target(value, "save-buffer")?);
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
        target: target.ok_or_else(|| "save-buffer requires -t <session>".to_string())?,
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
    let mut target = None;
    let mut buffer = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "paste-buffer requires a session name after -t".to_string())?;
                target = Some(parse_structured_target(value, "paste-buffer")?);
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
        target: target.ok_or_else(|| "paste-buffer requires -t <session>".to_string())?,
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

fn parse_run(args: Vec<String>) -> Result<Command, String> {
    let sequence = args.join(" ");
    if sequence.trim().is_empty() {
        return Err("run requires a command sequence".to_string());
    }
    Ok(Command::Run { sequence })
}

fn parse_source_file(args: Vec<String>) -> Result<Command, String> {
    match args.as_slice() {
        [path] => Ok(Command::SourceFile { path: path.clone() }),
        [] => Err("source-file requires a path".to_string()),
        _ => Err("source-file accepts exactly one path".to_string()),
    }
}

fn parse_run_shell(args: Vec<String>) -> Result<Command, String> {
    let command = args.join(" ");
    if command.trim().is_empty() {
        return Err("run-shell requires a shell command".to_string());
    }
    Ok(Command::RunShell { command })
}

fn parse_list_keys(args: Vec<String>) -> Result<Command, String> {
    parse_optional_format(args, "list-keys").map(|format| Command::ListKeys { format })
}

fn parse_bind_key(args: Vec<String>) -> Result<Command, String> {
    let [key, command @ ..] = args.as_slice() else {
        return Err("bind-key requires a key and command".to_string());
    };
    let key = crate::config::canonical_key(key)?;
    let command = crate::config::validate_binding_command(&command.join(" "))?;
    Ok(Command::BindKey { key, command })
}

fn parse_unbind_key(args: Vec<String>) -> Result<Command, String> {
    match args.as_slice() {
        [key] => Ok(Command::UnbindKey {
            key: crate::config::canonical_key(key)?,
        }),
        [] => Err("unbind-key requires a key".to_string()),
        _ => Err("unbind-key accepts exactly one key".to_string()),
    }
}

fn parse_show_options(args: Vec<String>) -> Result<Command, String> {
    parse_optional_format(args, "show-options").map(|format| Command::ShowOptions { format })
}

fn parse_set_option(args: Vec<String>) -> Result<Command, String> {
    match args.as_slice() {
        [name, value] => Ok(Command::SetOption {
            name: name.clone(),
            value: crate::config::validate_option_value(name, value)?,
        }),
        [] | [_] => Err("set-option requires an option name and value".to_string()),
        _ => Err("set-option accepts exactly an option name and value".to_string()),
    }
}

fn parse_optional_format(args: Vec<String>, command_name: &str) -> Result<Option<String>, String> {
    let mut format = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-F" | "--format" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a format after {}", args[i]))?;
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
    Ok(format)
}

fn parse_resize_pane(args: Vec<String>) -> Result<Command, String> {
    let mut target = None;
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
                target = Some(parse_structured_target(value, "resize-pane")?);
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

    let target = target.ok_or_else(|| "resize-pane requires -t <session>".to_string())?;
    if directional.is_some() && (cols.is_some() || rows.is_some()) {
        return Err("resize-pane accepts either -x/-y or one of -L/-R/-U/-D".to_string());
    }
    let resize = if let Some(resize) = directional {
        resize
    } else {
        if target.window != WindowTarget::Active || target.pane != PaneTarget::Active {
            return Err(
                "resize-pane -x/-y does not accept an explicit window or pane target".to_string(),
            );
        }
        PaneResize::Absolute {
            cols: cols.ok_or_else(|| "resize-pane requires -x <cols>".to_string())?,
            rows: rows.ok_or_else(|| "resize-pane requires -y <rows>".to_string())?,
        }
    };

    Ok(Command::ResizePane { target, resize })
}

fn parse_select_layout(args: Vec<String>) -> Result<Command, String> {
    let mut target = None;
    let mut preset = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "select-layout requires a session name after -t".to_string())?;
                target = Some(parse_structured_target(value, "select-layout")?);
                i += 2;
            }
            value if value.starts_with('-') => {
                return Err(format!("select-layout does not support option {value:?}"));
            }
            value => {
                if preset
                    .replace(crate::protocol::parse_layout_preset_name(value)?)
                    .is_some()
                {
                    return Err("select-layout accepts exactly one preset".to_string());
                }
                i += 1;
            }
        }
    }

    let target = target.ok_or_else(|| "select-layout requires -t <session>".to_string())?;
    if target.pane != PaneTarget::Active {
        return Err("select-layout target must not include a pane".to_string());
    }
    Ok(Command::SelectLayout {
        session: target.session,
        window: target.window,
        preset: preset.ok_or_else(|| "select-layout requires <preset>".to_string())?,
    })
}

fn parse_send_keys(args: Vec<String>) -> Result<Command, String> {
    let mut target = None;
    let mut keys = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "send-keys requires a session name after -t".to_string())?;
                target = Some(parse_structured_target(value, "send-keys")?);
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
        target: target.ok_or_else(|| "send-keys requires -t <session>".to_string())?,
        keys,
    })
}

fn parse_split_window(args: Vec<String>) -> Result<Command, String> {
    let mut target = None;
    let mut direction = None;
    let mut command = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "split-window requires a session name after -t".to_string())?;
                target = Some(parse_structured_target(value, "split-window")?);
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
        target: target.ok_or_else(|| "split-window requires -t <session>".to_string())?,
        direction: direction.ok_or_else(|| "split-window requires one of -h or -v".to_string())?,
        command,
    })
}

fn parse_list_panes(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut window = WindowTarget::Active;
    let mut format = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "list-panes requires a session name after -t".to_string())?;
                let target = parse_structured_target(value, "list-panes")?;
                if target.pane != PaneTarget::Active {
                    return Err("list-panes target must not include a pane".to_string());
                }
                session = Some(target.session);
                window = target.window;
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
        window,
        format,
    })
}

fn parse_select_pane(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut window = WindowTarget::Active;
    let mut target = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "select-pane requires a session name after -t".to_string())?;
                let parsed = parse_structured_target(value, "select-pane")?;
                session = Some(parsed.session);
                window = parsed.window;
                match parsed.pane {
                    PaneTarget::Active => {}
                    PaneTarget::Index(index) => {
                        set_pane_select_target(&mut target, PaneSelectTarget::Index(index))?;
                    }
                    PaneTarget::Id(id) => {
                        set_pane_select_target(&mut target, PaneSelectTarget::Id(id))?;
                    }
                }
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
        window,
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
    let mut target = None;
    let mut pane = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "kill-pane requires a session name after -t".to_string())?;
                target = Some(parse_structured_target(value, "kill-pane")?);
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
        target: apply_pane_index_target(
            target.ok_or_else(|| "kill-pane requires -t <session>".to_string())?,
            pane,
            "kill-pane",
        )?,
    })
}

fn parse_swap_pane(args: Vec<String>) -> Result<Command, String> {
    let mut source = None;
    let mut destination = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-s" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "swap-pane requires a source target after -s".to_string())?;
                source = Some(parse_structured_target(value, "swap-pane")?);
                i += 2;
            }
            "-t" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    "swap-pane requires a destination target after -t".to_string()
                })?;
                destination = Some(parse_structured_target(value, "swap-pane")?);
                i += 2;
            }
            value => return Err(format!("swap-pane does not support argument {value:?}")),
        }
    }

    Ok(Command::SwapPane {
        source: source.ok_or_else(|| "swap-pane requires -s <source-target>".to_string())?,
        destination: destination
            .ok_or_else(|| "swap-pane requires -t <destination-target>".to_string())?,
    })
}

fn parse_move_pane(args: Vec<String>) -> Result<Command, String> {
    let (source, destination, direction) = parse_pane_transfer_args(args, "move-pane")?;
    Ok(Command::MovePane {
        source,
        destination,
        direction,
    })
}

fn parse_break_pane(args: Vec<String>) -> Result<Command, String> {
    let mut target = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "break-pane requires a target after -t".to_string())?;
                target = Some(parse_structured_target(value, "break-pane")?);
                i += 2;
            }
            value => return Err(format!("break-pane does not support argument {value:?}")),
        }
    }

    Ok(Command::BreakPane {
        target: target.ok_or_else(|| "break-pane requires -t <target>".to_string())?,
    })
}

fn parse_join_pane(args: Vec<String>) -> Result<Command, String> {
    let (source, destination, direction) = parse_pane_transfer_args(args, "join-pane")?;
    Ok(Command::JoinPane {
        source,
        destination,
        direction,
    })
}

fn parse_pane_transfer_args(
    args: Vec<String>,
    command: &str,
) -> Result<(Target, Target, SplitDirection), String> {
    let mut source = None;
    let mut destination = None;
    let mut direction = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-s" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command} requires a source target after -s"))?;
                source = Some(parse_structured_target(value, command)?);
                i += 2;
            }
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command} requires a destination target after -t"))?;
                destination = Some(parse_structured_target(value, command)?);
                i += 2;
            }
            "-h" => {
                set_pane_transfer_direction(&mut direction, SplitDirection::Horizontal, command)?;
                i += 1;
            }
            "-v" => {
                set_pane_transfer_direction(&mut direction, SplitDirection::Vertical, command)?;
                i += 1;
            }
            value => return Err(format!("{command} does not support argument {value:?}")),
        }
    }

    Ok((
        source.ok_or_else(|| format!("{command} requires -s <source-target>"))?,
        destination.ok_or_else(|| format!("{command} requires -t <destination-target>"))?,
        direction.unwrap_or(SplitDirection::Horizontal),
    ))
}

fn set_pane_transfer_direction(
    direction: &mut Option<SplitDirection>,
    value: SplitDirection,
    command: &str,
) -> Result<(), String> {
    if direction.replace(value).is_some() {
        Err(format!("{command} accepts only one direction"))
    } else {
        Ok(())
    }
}

fn parse_respawn_pane(args: Vec<String>) -> Result<Command, String> {
    let mut target = None;
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
                target = Some(parse_structured_target(value, "respawn-pane")?);
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
        target: apply_pane_index_target(
            target.ok_or_else(|| "respawn-pane requires -t <session>".to_string())?,
            pane,
            "respawn-pane",
        )?,
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
    let mut target = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| format!("{command_name} requires a session name after -t"))?;
                let parsed = parse_structured_target(value, command_name)?;
                if parsed.pane != PaneTarget::Active {
                    return Err(format!("{command_name} target must not include a pane"));
                }
                session = Some(parsed.session);
                if parsed.window != WindowTarget::Active {
                    target = Some(parsed.window);
                }
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
            "--window-id" | "--tab-id" => {
                let value = args.get(i + 1).ok_or_else(|| {
                    format!("{command_name} requires a window id after {}", args[i])
                })?;
                set_window_target(
                    &mut target,
                    WindowTarget::Id(value.parse::<usize>().map_err(|_| {
                        format!("{command_name} {} must be a non-negative integer", args[i])
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

    Ok(Command::KillWindow {
        session: session.ok_or_else(|| format!("{command_name} requires -t <session>"))?,
        target: target.unwrap_or(WindowTarget::Active),
    })
}

fn parse_zoom_pane(args: Vec<String>) -> Result<Command, String> {
    let mut target = None;
    let mut pane = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "zoom-pane requires a session name after -t".to_string())?;
                target = Some(parse_structured_target(value, "zoom-pane")?);
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
        target: apply_pane_index_target(
            target.ok_or_else(|| "zoom-pane requires -t <session>".to_string())?,
            pane,
            "zoom-pane",
        )?,
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

fn parse_structured_target(value: &str, command: &str) -> Result<Target, String> {
    let (session, rest) = value
        .split_once(':')
        .map_or((value, None), |(session, rest)| (session, Some(rest)));
    if session.is_empty() {
        return Err(format!("{command} target requires a session name"));
    }
    let mut target = Target::active(session.to_string());
    let Some(rest) = rest else {
        return Ok(target);
    };
    if rest.is_empty() {
        return Err(format!(
            "{command} target requires a window or pane after ':'"
        ));
    }
    let (window, pane) = rest
        .split_once('.')
        .map_or((rest, None), |(window, pane)| (window, Some(pane)));
    if pane.is_none()
        && !window.starts_with('@')
        && !window.starts_with('=')
        && window.parse::<usize>().is_err()
    {
        return Ok(Target::active(value.to_string()));
    }
    if !window.is_empty() {
        target.window = parse_window_target_token(window, command)?;
    }
    if let Some(pane) = pane {
        if pane.is_empty() {
            return Err(format!("{command} target requires a pane after '.'"));
        }
        target.pane = parse_pane_target_token(pane, command)?;
    }
    Ok(target)
}

fn parse_window_target_token(value: &str, command: &str) -> Result<WindowTarget, String> {
    if let Some(id) = value.strip_prefix('@') {
        return id
            .parse::<usize>()
            .map(WindowTarget::Id)
            .map_err(|_| format!("{command} target has invalid window id"));
    }
    if let Some(name) = value.strip_prefix('=') {
        if name.is_empty() {
            return Err(format!("{command} target has invalid window name"));
        }
        return Ok(WindowTarget::Name(name.to_string()));
    }
    if let Ok(index) = value.parse::<usize>() {
        return Ok(WindowTarget::Index(index));
    }
    Ok(WindowTarget::Name(value.to_string()))
}

fn parse_pane_target_token(value: &str, command: &str) -> Result<PaneTarget, String> {
    if let Some(id) = value.strip_prefix('%') {
        return id
            .parse::<usize>()
            .map(PaneTarget::Id)
            .map_err(|_| format!("{command} target has invalid pane id"));
    }
    value
        .parse::<usize>()
        .map(PaneTarget::Index)
        .map_err(|_| format!("{command} target has invalid pane index"))
}

fn apply_pane_index_target(
    mut target: Target,
    pane: Option<usize>,
    command: &str,
) -> Result<Target, String> {
    if let Some(index) = pane {
        if target.pane != PaneTarget::Active {
            return Err(format!("{command} accepts exactly one pane target"));
        }
        target.pane = PaneTarget::Index(index);
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_target(session: &str) -> Target {
        Target::active(session.to_string())
    }

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
    fn parses_batch_execution_commands() {
        assert_eq!(
            parse_args(["dmux", "run", "new -d -s dev; split-window -t dev -v"]).unwrap(),
            Command::Run {
                sequence: "new -d -s dev; split-window -t dev -v".to_string(),
            }
        );
        assert_eq!(
            parse_args(["dmux", "command", "ls"]).unwrap(),
            Command::Run {
                sequence: "ls".to_string(),
            }
        );
        assert_eq!(
            parse_args(["dmux", "source-file", "dmux.commands"]).unwrap(),
            Command::SourceFile {
                path: "dmux.commands".to_string(),
            }
        );
        assert_eq!(
            parse_args(["dmux", "run-shell", "printf", "ok"]).unwrap(),
            Command::RunShell {
                command: "printf ok".to_string(),
            }
        );
    }

    #[test]
    fn parses_key_binding_and_option_commands() {
        assert_eq!(
            parse_args(["dmux", "list-keys", "-F", "#{key}=#{command}"]).unwrap(),
            Command::ListKeys {
                format: Some("#{key}=#{command}".to_string()),
            }
        );
        assert_eq!(
            parse_args(["dmux", "bind-key", "C-a", "copy-mode"]).unwrap(),
            Command::BindKey {
                key: "C-a".to_string(),
                command: "copy-mode".to_string(),
            }
        );
        assert_eq!(
            parse_args(["dmux", "unbind-key", "x"]).unwrap(),
            Command::UnbindKey {
                key: "x".to_string(),
            }
        );
        assert_eq!(
            parse_args(["dmux", "show-options"]).unwrap(),
            Command::ShowOptions { format: None }
        );
        assert_eq!(
            parse_args(["dmux", "set-option", "prefix", "C-a"]).unwrap(),
            Command::SetOption {
                name: "prefix".to_string(),
                value: "C-a".to_string(),
            }
        );
    }

    #[test]
    fn rejects_invalid_binding_and_option_values() {
        let err = parse_args(["dmux", "bind-key", "x", "run-shell", "date"]).unwrap_err();
        assert!(err.contains("unsupported binding command"), "{err}");

        let err = parse_args(["dmux", "set-option", "prefix", "bad-key"]).unwrap_err();
        assert!(err.contains("invalid key"), "{err}");
    }

    #[test]
    fn command_sequence_parser_supports_quotes_and_escaping() {
        let commands = parse_command_sequence(
            "display-message -t dev -p 'hello; world'; rename-window -t dev \"api server\"",
        )
        .unwrap();

        assert_eq!(
            commands,
            vec![
                ScriptCommand {
                    source: "display-message -t dev -p 'hello; world'".to_string(),
                    argv: vec![
                        "display-message".to_string(),
                        "-t".to_string(),
                        "dev".to_string(),
                        "-p".to_string(),
                        "hello; world".to_string(),
                    ],
                },
                ScriptCommand {
                    source: "rename-window -t dev \"api server\"".to_string(),
                    argv: vec![
                        "rename-window".to_string(),
                        "-t".to_string(),
                        "dev".to_string(),
                        "api server".to_string(),
                    ],
                },
            ]
        );

        assert_eq!(
            tokenize_command(r#"send-keys -t dev hello\ world"#).unwrap(),
            vec![
                "send-keys".to_string(),
                "-t".to_string(),
                "dev".to_string(),
                "hello world".to_string(),
            ]
        );

        let shell_command =
            parse_command_sequence(r"run-shell printf '%s\n' '\'; display-message -t dev ok")
                .unwrap();
        assert_eq!(shell_command.len(), 2);
        assert_eq!(shell_command[0].source, r"run-shell printf '%s\n' '\'");
        assert_eq!(shell_command[0].argv[3], "\\");
    }

    #[test]
    fn command_file_parser_skips_blank_lines_and_comments() {
        let commands = parse_command_file(
            "\n# setup\nnew -d -s dev\n  \nrename-window -t dev api; list-windows -t dev\n",
        )
        .unwrap();

        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0].line, 3);
        assert_eq!(commands[0].command.argv[0], "new");
        assert_eq!(commands[1].line, 5);
        assert_eq!(commands[1].command.argv[0], "rename-window");
        assert_eq!(commands[2].line, 5);
        assert_eq!(commands[2].command.argv[0], "list-windows");
    }

    #[test]
    fn command_file_parser_reports_line_context_for_parse_errors() {
        let err = parse_command_file("new -d -s dev\nrename-window 'unterminated\n").unwrap_err();

        assert!(err.contains("line 2"), "{err}");
        assert!(err.contains("unterminated"), "{err}");
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
        assert!(help.contains("semicolon-separated"), "{help}");
        assert!(help.contains("Session:"), "{help}");
        assert!(help.contains("Prompt examples:"), "{help}");
        assert!(help.contains("copy-mode:"), "{help}");
    }

    #[test]
    fn attach_help_overlay_groups_common_workflows() {
        let help = attach_help_overlay();

        assert!(help.contains("Session:"), "{help}");
        assert!(help.contains("Windows:"), "{help}");
        assert!(help.contains("Panes:"), "{help}");
        assert!(help.contains("Copy:"), "{help}");
        assert!(help.contains("Prompt examples:"), "{help}");
        assert!(help.contains(":split -h"), "{help}");
    }

    #[test]
    fn parses_capture_pane_print() {
        let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p"]).unwrap();
        assert_eq!(
            command,
            Command::CapturePane {
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
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
                target: active_target("dev"),
                resize: PaneResize::Directional {
                    direction: PaneResizeDirection::Down,
                    amount: 5,
                },
            }
        );
    }

    #[test]
    fn parses_select_layout_structured_window_target() {
        let command = parse_args(["dmux", "select-layout", "-t", "dev:@7", "tiled"]).unwrap();
        assert_eq!(
            command,
            Command::SelectLayout {
                session: "dev".to_string(),
                window: WindowTarget::Id(7),
                preset: crate::protocol::LayoutPreset::Tiled,
            }
        );
    }

    #[test]
    fn rejects_select_layout_pane_target() {
        let err = parse_args(["dmux", "select-layout", "-t", "dev:1.%2", "tiled"]).unwrap_err();
        assert!(err.contains("must not include a pane"), "{err}");
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
                target: active_target("dev"),
                keys: vec!["echo hi".to_string(), "Enter".to_string()],
            }
        );
    }

    #[test]
    fn parses_structured_pane_target() {
        let command = parse_args(["dmux", "send-keys", "-t", "dev:@7.%42", "Enter"]).unwrap();
        assert_eq!(
            command,
            Command::SendKeys {
                target: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Id(7),
                    pane: PaneTarget::Id(42),
                },
                keys: vec!["Enter".to_string()],
            }
        );
    }

    #[test]
    fn rejects_invalid_structured_pane_target() {
        let err = parse_args(["dmux", "send-keys", "-t", "dev:1.bad", "Enter"]).unwrap_err();
        assert!(err.contains("invalid pane index"), "{err}");
    }

    #[test]
    fn preserves_colon_session_name_without_structured_target() {
        let command = parse_args(["dmux", "send-keys", "-t", "dev:api", "Enter"]).unwrap();
        assert_eq!(
            command,
            Command::SendKeys {
                target: Target::active("dev:api".to_string()),
                keys: vec!["Enter".to_string()],
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
                target: active_target("dev"),
                direction: crate::protocol::SplitDirection::Horizontal,
                command: vec!["sh".to_string(), "-c".to_string(), "echo split".to_string()],
            }
        );
    }

    #[test]
    fn parses_split_window_structured_target() {
        let command = parse_args(["dmux", "split-window", "-t", "dev:1.%7", "-v"]).unwrap();

        assert_eq!(
            command,
            Command::SplitWindow {
                target: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Index(1),
                    pane: PaneTarget::Id(7),
                },
                direction: crate::protocol::SplitDirection::Vertical,
                command: Vec::new(),
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
                window: WindowTarget::Active,
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
                window: WindowTarget::Active,
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
                window: WindowTarget::Active,
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
                window: WindowTarget::Active,
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
                window: WindowTarget::Active,
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
                target: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Active,
                    pane: PaneTarget::Index(1),
                },
            }
        );
    }

    #[test]
    fn parses_kill_pane_target_without_index() {
        let command = parse_args(["dmux", "kill-pane", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::KillPane {
                target: active_target("dev"),
            }
        );
    }

    #[test]
    fn parses_swap_pane_source_and_destination_targets() {
        let command =
            parse_args(["dmux", "swap-pane", "-s", "dev:0.%10", "-t", "dev:0.2"]).unwrap();
        assert_eq!(
            command,
            Command::SwapPane {
                source: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Index(0),
                    pane: PaneTarget::Id(10),
                },
                destination: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Index(0),
                    pane: PaneTarget::Index(2),
                },
            }
        );
    }

    #[test]
    fn parses_move_break_and_join_pane_targets() {
        let move_command = parse_args([
            "dmux",
            "move-pane",
            "-s",
            "dev:0.%10",
            "-t",
            "dev:1.0",
            "-v",
        ])
        .unwrap();
        assert_eq!(
            move_command,
            Command::MovePane {
                source: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Index(0),
                    pane: PaneTarget::Id(10),
                },
                destination: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Index(1),
                    pane: PaneTarget::Index(0),
                },
                direction: SplitDirection::Vertical,
            }
        );

        let break_command = parse_args(["dmux", "break-pane", "-t", "dev:0.%10"]).unwrap();
        assert_eq!(
            break_command,
            Command::BreakPane {
                target: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Index(0),
                    pane: PaneTarget::Id(10),
                },
            }
        );

        let join_command =
            parse_args(["dmux", "join-pane", "-s", "dev:1.0", "-t", "dev:0.%10"]).unwrap();
        assert_eq!(
            join_command,
            Command::JoinPane {
                source: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Index(1),
                    pane: PaneTarget::Index(0),
                },
                destination: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Index(0),
                    pane: PaneTarget::Id(10),
                },
                direction: SplitDirection::Horizontal,
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
                target: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Active,
                    pane: PaneTarget::Index(1),
                },
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
                target: WindowTarget::Index(1),
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
                target: WindowTarget::Index(1),
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
                target: WindowTarget::Active,
            }
        );
    }

    #[test]
    fn parses_zoom_pane_target_without_index() {
        let command = parse_args(["dmux", "zoom-pane", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::ZoomPane {
                target: active_target("dev"),
            }
        );
    }

    #[test]
    fn parses_zoom_pane_target_and_index() {
        let command = parse_args(["dmux", "zoom-pane", "-t", "dev", "-p", "0"]).unwrap();
        assert_eq!(
            command,
            Command::ZoomPane {
                target: Target {
                    session: "dev".to_string(),
                    window: WindowTarget::Active,
                    pane: PaneTarget::Index(0),
                },
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
