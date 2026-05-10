pub const ARG_SEPARATOR: char = '\u{1f}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    New {
        session: String,
        command: Vec<String>,
    },
    Attach {
        session: String,
    },
    List,
    Capture {
        session: String,
    },
    Resize {
        session: String,
        cols: u16,
        rows: u16,
    },
    Send {
        session: String,
        bytes: Vec<u8>,
    },
    Split {
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
    Kill {
        session: String,
    },
    KillServer,
}

pub fn encode_new(session: &str, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!("NEW\t{session}\t{}\t{joined}\n", command.len())
}

pub fn encode_attach(session: &str) -> String {
    format!("ATTACH\t{session}\n")
}

pub fn encode_list() -> &'static str {
    "LIST\n"
}

pub fn encode_capture(session: &str) -> String {
    format!("CAPTURE\t{session}\n")
}

pub fn encode_resize(session: &str, cols: u16, rows: u16) -> String {
    format!("RESIZE\t{session}\t{cols}\t{rows}\n")
}

pub fn encode_send(session: &str, bytes: &[u8]) -> String {
    format!("SEND\t{session}\t{}\n", encode_hex(bytes))
}

pub fn encode_split(session: &str, direction: SplitDirection, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!(
        "SPLIT\t{session}\t{}\t{}\t{joined}\n",
        encode_split_direction(direction),
        command.len()
    )
}

pub fn encode_list_panes(session: &str, format: Option<&str>) -> String {
    match format {
        Some(format) => format!(
            "LIST_PANES_FORMAT\t{session}\t{}\n",
            encode_hex(format.as_bytes())
        ),
        None => format!("LIST_PANES\t{session}\n"),
    }
}

pub fn encode_select_pane(session: &str, pane: usize) -> String {
    format!("SELECT_PANE\t{session}\t{pane}\n")
}

pub fn encode_kill_pane(session: &str, pane: Option<usize>) -> String {
    match pane {
        Some(pane) => format!("KILL_PANE\t{session}\t{pane}\n"),
        None => format!("KILL_PANE\t{session}\tactive\n"),
    }
}

pub fn encode_new_window(session: &str, command: &[String]) -> String {
    let joined = command.join(&ARG_SEPARATOR.to_string());
    format!("NEW_WINDOW\t{session}\t{}\t{joined}\n", command.len())
}

pub fn encode_list_windows(session: &str) -> String {
    format!("LIST_WINDOWS\t{session}\n")
}

pub fn encode_select_window(session: &str, window: usize) -> String {
    format!("SELECT_WINDOW\t{session}\t{window}\n")
}

pub fn encode_kill_window(session: &str, window: Option<usize>) -> String {
    match window {
        Some(window) => format!("KILL_WINDOW\t{session}\t{window}\n"),
        None => format!("KILL_WINDOW\t{session}\tactive\n"),
    }
}

pub fn encode_zoom_pane(session: &str, pane: Option<usize>) -> String {
    match pane {
        Some(pane) => format!("ZOOM_PANE\t{session}\t{pane}\n"),
        None => format!("ZOOM_PANE\t{session}\tactive\n"),
    }
}

pub fn encode_status_line(session: &str, format: Option<&str>) -> String {
    match format {
        Some(format) => format!(
            "STATUS_LINE_FORMAT\t{session}\t{}\n",
            encode_hex(format.as_bytes())
        ),
        None => format!("STATUS_LINE\t{session}\n"),
    }
}

pub fn encode_display_message(session: &str, format: &str) -> String {
    format!(
        "DISPLAY_MESSAGE\t{session}\t{}\n",
        encode_hex(format.as_bytes())
    )
}

pub fn encode_kill(session: &str) -> String {
    format!("KILL\t{session}\n")
}

pub fn encode_kill_server() -> &'static str {
    "KILL_SERVER\n"
}

pub fn decode_request(line: &str) -> Result<Request, String> {
    let line = line.trim_end_matches('\n');
    let parts = line.split('\t').collect::<Vec<_>>();

    match parts.as_slice() {
        ["NEW", session, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "NEW has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("NEW argc does not match command".to_string());
            }
            Ok(Request::New {
                session: (*session).to_string(),
                command,
            })
        }
        ["ATTACH", session] => Ok(Request::Attach {
            session: (*session).to_string(),
        }),
        ["LIST"] => Ok(Request::List),
        ["CAPTURE", session] => Ok(Request::Capture {
            session: (*session).to_string(),
        }),
        ["RESIZE", session, cols, rows] => Ok(Request::Resize {
            session: (*session).to_string(),
            cols: cols
                .parse::<u16>()
                .map_err(|_| "RESIZE has invalid cols".to_string())?,
            rows: rows
                .parse::<u16>()
                .map_err(|_| "RESIZE has invalid rows".to_string())?,
        }),
        ["SEND", session, hex] => Ok(Request::Send {
            session: (*session).to_string(),
            bytes: decode_hex(hex)?,
        }),
        ["SPLIT", session, direction, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "SPLIT has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("SPLIT argc does not match command".to_string());
            }
            Ok(Request::Split {
                session: (*session).to_string(),
                direction: decode_split_direction(direction)?,
                command,
            })
        }
        ["LIST_PANES", session] => Ok(Request::ListPanes {
            session: (*session).to_string(),
            format: None,
        }),
        ["LIST_PANES_FORMAT", session, format] => Ok(Request::ListPanes {
            session: (*session).to_string(),
            format: Some(decode_utf8_hex(format, "LIST_PANES_FORMAT")?),
        }),
        ["SELECT_PANE", session, pane] => Ok(Request::SelectPane {
            session: (*session).to_string(),
            pane: pane
                .parse::<usize>()
                .map_err(|_| "SELECT_PANE has invalid pane index".to_string())?,
        }),
        ["KILL_PANE", session, pane] => Ok(Request::KillPane {
            session: (*session).to_string(),
            pane: decode_optional_pane(pane)?,
        }),
        ["NEW_WINDOW", session, argc, joined] => {
            let argc = argc
                .parse::<usize>()
                .map_err(|_| "NEW_WINDOW has invalid argc".to_string())?;
            let command = if *joined == "" {
                Vec::new()
            } else {
                joined.split(ARG_SEPARATOR).map(str::to_string).collect()
            };
            if command.len() != argc {
                return Err("NEW_WINDOW argc does not match command".to_string());
            }
            Ok(Request::NewWindow {
                session: (*session).to_string(),
                command,
            })
        }
        ["LIST_WINDOWS", session] => Ok(Request::ListWindows {
            session: (*session).to_string(),
        }),
        ["SELECT_WINDOW", session, window] => Ok(Request::SelectWindow {
            session: (*session).to_string(),
            window: window
                .parse::<usize>()
                .map_err(|_| "SELECT_WINDOW has invalid window index".to_string())?,
        }),
        ["KILL_WINDOW", session, window] => Ok(Request::KillWindow {
            session: (*session).to_string(),
            window: decode_optional_window(window)?,
        }),
        ["ZOOM_PANE", session, pane] => Ok(Request::ZoomPane {
            session: (*session).to_string(),
            pane: decode_optional_zoom_pane(pane)?,
        }),
        ["STATUS_LINE", session] => Ok(Request::StatusLine {
            session: (*session).to_string(),
            format: None,
        }),
        ["STATUS_LINE_FORMAT", session, format] => Ok(Request::StatusLine {
            session: (*session).to_string(),
            format: Some(decode_utf8_hex(format, "STATUS_LINE_FORMAT")?),
        }),
        ["DISPLAY_MESSAGE", session, format] => Ok(Request::DisplayMessage {
            session: (*session).to_string(),
            format: decode_utf8_hex(format, "DISPLAY_MESSAGE")?,
        }),
        ["KILL", session] => Ok(Request::Kill {
            session: (*session).to_string(),
        }),
        ["KILL_SERVER"] => Ok(Request::KillServer),
        _ => Err(format!("unknown request line: {line:?}")),
    }
}

fn encode_split_direction(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Horizontal => "h",
        SplitDirection::Vertical => "v",
    }
}

fn decode_split_direction(value: &str) -> Result<SplitDirection, String> {
    match value {
        "h" => Ok(SplitDirection::Horizontal),
        "v" => Ok(SplitDirection::Vertical),
        _ => Err("SPLIT has invalid direction".to_string()),
    }
}

fn decode_optional_pane(value: &str) -> Result<Option<usize>, String> {
    decode_optional_index(value, "KILL_PANE has invalid pane index")
}

fn decode_optional_zoom_pane(value: &str) -> Result<Option<usize>, String> {
    decode_optional_index(value, "ZOOM_PANE has invalid pane index")
}

fn decode_optional_window(value: &str) -> Result<Option<usize>, String> {
    decode_optional_index(value, "KILL_WINDOW has invalid window index")
}

fn decode_optional_index(value: &str, invalid_message: &str) -> Result<Option<usize>, String> {
    if value == "active" {
        Ok(None)
    } else {
        value
            .parse::<usize>()
            .map(Some)
            .map_err(|_| invalid_message.to_string())
    }
}

fn decode_utf8_hex(hex: &str, command: &str) -> Result<String, String> {
    String::from_utf8(decode_hex(hex)?).map_err(|_| format!("{command} has non-utf8 format"))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("hex payload has odd length".to_string());
    }

    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let hi = hex_value(pair[0])?;
            let lo = hex_value(pair[1])?;
            Ok((hi << 4) | lo)
        })
        .collect()
}

fn hex_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("hex payload contains non-hex byte".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_new_request_with_spaced_args() {
        let command = vec!["sh".to_string(), "-c".to_string(), "echo ok".to_string()];
        let line = encode_new("dev", &command);

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::New {
                session: "dev".to_string(),
                command
            }
        );
    }

    #[test]
    fn round_trips_resize_request() {
        let line = encode_resize("dev", 100, 40);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Resize {
                session: "dev".to_string(),
                cols: 100,
                rows: 40,
            }
        );
    }

    #[test]
    fn round_trips_send_request() {
        let line = encode_send("dev", b"hello\r");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Send {
                session: "dev".to_string(),
                bytes: b"hello\r".to_vec(),
            }
        );
    }

    #[test]
    fn round_trips_split_request() {
        let command = vec!["sh".to_string(), "-c".to_string(), "echo split".to_string()];
        let line = encode_split("dev", SplitDirection::Horizontal, &command);

        assert_eq!(
            decode_request(&line).unwrap(),
            Request::Split {
                session: "dev".to_string(),
                direction: SplitDirection::Horizontal,
                command,
            }
        );
    }

    #[test]
    fn round_trips_select_pane_request() {
        let line = encode_select_pane("dev", 1);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SelectPane {
                session: "dev".to_string(),
                pane: 1,
            }
        );
    }

    #[test]
    fn round_trips_kill_pane_request_with_index() {
        let line = encode_kill_pane("dev", Some(1));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::KillPane {
                session: "dev".to_string(),
                pane: Some(1),
            }
        );
    }

    #[test]
    fn round_trips_kill_pane_request_for_active_pane() {
        let line = encode_kill_pane("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::KillPane {
                session: "dev".to_string(),
                pane: None,
            }
        );
    }

    #[test]
    fn round_trips_new_window_request() {
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo window".to_string(),
        ];
        let line = encode_new_window("dev", &command);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::NewWindow {
                session: "dev".to_string(),
                command,
            }
        );
    }

    #[test]
    fn round_trips_select_window_request() {
        let line = encode_select_window("dev", 1);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::SelectWindow {
                session: "dev".to_string(),
                window: 1,
            }
        );
    }

    #[test]
    fn round_trips_kill_window_request_with_index() {
        let line = encode_kill_window("dev", Some(1));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::KillWindow {
                session: "dev".to_string(),
                window: Some(1),
            }
        );
    }

    #[test]
    fn round_trips_kill_window_request_for_active_window() {
        let line = encode_kill_window("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::KillWindow {
                session: "dev".to_string(),
                window: None,
            }
        );
    }

    #[test]
    fn round_trips_zoom_pane_request_for_active_pane() {
        let line = encode_zoom_pane("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ZoomPane {
                session: "dev".to_string(),
                pane: None,
            }
        );
    }

    #[test]
    fn round_trips_zoom_pane_request_with_index() {
        let line = encode_zoom_pane("dev", Some(0));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ZoomPane {
                session: "dev".to_string(),
                pane: Some(0),
            }
        );
    }

    #[test]
    fn round_trips_list_panes_format_request() {
        let line = encode_list_panes("dev", Some("#{pane.index}:#{pane.zoomed}"));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::ListPanes {
                session: "dev".to_string(),
                format: Some("#{pane.index}:#{pane.zoomed}".to_string()),
            }
        );
    }

    #[test]
    fn rejects_invalid_kill_pane_index_with_existing_message() {
        let err = decode_request("KILL_PANE\tdev\tbad\n").unwrap_err();
        assert_eq!(err, "KILL_PANE has invalid pane index");
    }

    #[test]
    fn rejects_invalid_kill_window_index() {
        let err = decode_request("KILL_WINDOW\tdev\tbad\n").unwrap_err();
        assert_eq!(err, "KILL_WINDOW has invalid window index");
    }

    #[test]
    fn rejects_invalid_zoom_pane_index() {
        let err = decode_request("ZOOM_PANE\tdev\tbad\n").unwrap_err();
        assert_eq!(err, "ZOOM_PANE has invalid pane index");
    }

    #[test]
    fn round_trips_status_line_request() {
        let line = encode_status_line("dev", None);
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::StatusLine {
                session: "dev".to_string(),
                format: None,
            }
        );
    }

    #[test]
    fn round_trips_status_line_format_request() {
        let line = encode_status_line("dev", Some("#{session.name}:#{pane.index}"));
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::StatusLine {
                session: "dev".to_string(),
                format: Some("#{session.name}:#{pane.index}".to_string()),
            }
        );
    }

    #[test]
    fn round_trips_display_message_request() {
        let line = encode_display_message("dev", "#{window.list}");
        assert_eq!(
            decode_request(&line).unwrap(),
            Request::DisplayMessage {
                session: "dev".to_string(),
                format: "#{window.list}".to_string(),
            }
        );
    }
}
