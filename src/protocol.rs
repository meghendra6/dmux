pub const ARG_SEPARATOR: char = '\u{1f}';

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    New { session: String, command: Vec<String> },
    Attach { session: String },
    List,
    Capture { session: String },
    Kill { session: String },
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
        ["KILL", session] => Ok(Request::Kill {
            session: (*session).to_string(),
        }),
        ["KILL_SERVER"] => Ok(Request::KillServer),
        _ => Err(format!("unknown request line: {line:?}")),
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
}
