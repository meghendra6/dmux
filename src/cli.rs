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
    let session = parse_target(args, "list-panes")?
        .ok_or_else(|| "list-panes requires -t <session>".to_string())?;
    Ok(Command::ListPanes { session })
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
                session: "dev".to_string()
            }
        );
    }
}
