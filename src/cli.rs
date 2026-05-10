use crate::protocol::SplitDirection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    New {
        session: String,
        detach: bool,
        command: Vec<String>,
    },
    Attach {
        session: String,
    },
    ListSessions,
    CapturePane {
        session: String,
    },
    ResizePane {
        session: String,
        cols: u16,
        rows: u16,
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
        pane: usize,
    },
    KillPane {
        session: String,
        pane: Option<usize>,
    },
    NewWindow {
        session: String,
        command: Vec<String>,
    },
    ListWindows {
        session: String,
    },
    SelectWindow {
        session: String,
        window: usize,
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
    Server,
}

pub fn parse_args<I, S>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if args.is_empty() {
        return Ok(Command::Attach {
            session: "default".to_string(),
        });
    }

    let program = args.remove(0);
    let Some(subcommand) = args.first().cloned() else {
        return Ok(Command::Attach {
            session: "default".to_string(),
        });
    };
    args.remove(0);

    match subcommand.as_str() {
        "__server" => Ok(Command::Server),
        "new" | "new-session" => parse_new(args),
        "attach" | "attach-session" => parse_attach(args),
        "ls" | "list-sessions" => Ok(Command::ListSessions),
        "capture-pane" => parse_capture(args),
        "resize-pane" => parse_resize_pane(args),
        "send-keys" => parse_send_keys(args),
        "split-window" | "split" => parse_split_window(args),
        "list-panes" => parse_list_panes(args),
        "select-pane" => parse_select_pane(args),
        "kill-pane" => parse_kill_pane(args),
        "new-window" => parse_new_window(args),
        "list-windows" => parse_list_windows(args),
        "select-window" => parse_select_window(args),
        "kill-window" => parse_kill_window(args),
        "zoom-pane" => parse_zoom_pane(args),
        "status-line" => parse_status_line(args),
        "display-message" => parse_display_message(args),
        "kill-session" => parse_kill_session(args),
        "kill-server" => Ok(Command::KillServer),
        _ => Err(format!("{program}: unknown command {subcommand:?}")),
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
    Ok(Command::Attach {
        session: parse_target(args, "attach")?.unwrap_or_else(|| "default".to_string()),
    })
}

fn parse_capture(args: Vec<String>) -> Result<Command, String> {
    let session = parse_target(args, "capture-pane")?
        .ok_or_else(|| "capture-pane requires -t <session>".to_string())?;
    Ok(Command::CapturePane { session })
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
            value => return Err(format!("resize-pane does not support argument {value:?}")),
        }
    }

    Ok(Command::ResizePane {
        session: session.ok_or_else(|| "resize-pane requires -t <session>".to_string())?,
        cols: cols.ok_or_else(|| "resize-pane requires -x <cols>".to_string())?,
        rows: rows.ok_or_else(|| "resize-pane requires -y <rows>".to_string())?,
    })
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
    let mut pane = None;
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
                pane =
                    Some(value.parse::<usize>().map_err(|_| {
                        "select-pane -p must be a non-negative integer".to_string()
                    })?);
                i += 2;
            }
            value => return Err(format!("select-pane does not support argument {value:?}")),
        }
    }

    Ok(Command::SelectPane {
        session: session.ok_or_else(|| "select-pane requires -t <session>".to_string())?,
        pane: pane.ok_or_else(|| "select-pane requires -p <index>".to_string())?,
    })
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

fn parse_new_window(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut command = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "new-window requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "--" => {
                command.extend(args[i + 1..].iter().cloned());
                break;
            }
            value if value.starts_with('-') => {
                return Err(format!("new-window does not support option {value:?}"));
            }
            _ => {
                command.extend(args[i..].iter().cloned());
                break;
            }
        }
    }

    Ok(Command::NewWindow {
        session: session.ok_or_else(|| "new-window requires -t <session>".to_string())?,
        command,
    })
}

fn parse_list_windows(args: Vec<String>) -> Result<Command, String> {
    let session = parse_target(args, "list-windows")?
        .ok_or_else(|| "list-windows requires -t <session>".to_string())?;
    Ok(Command::ListWindows { session })
}

fn parse_select_window(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut window = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "select-window requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-w" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "select-window requires a window index after -w".to_string())?;
                window =
                    Some(value.parse::<usize>().map_err(|_| {
                        "select-window -w must be a non-negative integer".to_string()
                    })?);
                i += 2;
            }
            value => return Err(format!("select-window does not support argument {value:?}")),
        }
    }

    Ok(Command::SelectWindow {
        session: session.ok_or_else(|| "select-window requires -t <session>".to_string())?,
        window: window.ok_or_else(|| "select-window requires -w <index>".to_string())?,
    })
}

fn parse_kill_window(args: Vec<String>) -> Result<Command, String> {
    let mut session = None;
    let mut window = None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-t" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "kill-window requires a session name after -t".to_string())?;
                session = Some(value.clone());
                i += 2;
            }
            "-w" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| "kill-window requires a window index after -w".to_string())?;
                window =
                    Some(value.parse::<usize>().map_err(|_| {
                        "kill-window -w must be a non-negative integer".to_string()
                    })?);
                i += 2;
            }
            value => return Err(format!("kill-window does not support argument {value:?}")),
        }
    }

    Ok(Command::KillWindow {
        session: session.ok_or_else(|| "kill-window requires -t <session>".to_string())?,
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
    fn parses_capture_pane_print() {
        let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p"]).unwrap();
        assert_eq!(
            command,
            Command::CapturePane {
                session: "dev".to_string()
            }
        );
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
                cols: 100,
                rows: 40,
            }
        );
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
                pane: 1,
            }
        );
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
    fn parses_list_windows_target() {
        let command = parse_args(["dmux", "list-windows", "-t", "dev"]).unwrap();
        assert_eq!(
            command,
            Command::ListWindows {
                session: "dev".to_string()
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
                window: 1,
            }
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
