use crate::protocol;
use crate::pty::PtySize;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command as ProcessCommand, Stdio};

pub fn attach(socket: &Path, session: &str) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(protocol::encode_attach(session).as_bytes())?;

    let response = read_line(&mut stream)?;
    if let Some(message) = response.strip_prefix("ERR ") {
        return Err(io::Error::other(message.trim_end().to_string()));
    }
    if response != "OK\n" {
        return Err(io::Error::other(format!(
            "unexpected server response: {response:?}"
        )));
    }

    let _guard = RawModeGuard::enable();
    let mut output_stream = stream.try_clone()?;
    let output = std::thread::spawn(move || {
        let mut stdout = io::stdout().lock();
        let _ = io::copy(&mut output_stream, &mut stdout);
        let _ = stdout.flush();
    });

    forward_stdin_until_detach(&mut stream)?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = output.join();
    Ok(())
}

pub fn detect_attach_size() -> Option<PtySize> {
    if let Ok(value) = std::env::var("DEVMUX_ATTACH_SIZE") {
        return parse_attach_size(&value).ok();
    }

    if !stdin_is_tty() {
        return None;
    }

    let output = ProcessCommand::new("stty")
        .arg("size")
        .stdin(Stdio::inherit())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_stty_size(&String::from_utf8_lossy(&output.stdout)).ok()
}

fn parse_attach_size(value: &str) -> io::Result<PtySize> {
    let (cols, rows) = value
        .split_once('x')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "expected <cols>x<rows>"))?;
    let cols = cols
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid columns"))?;
    let rows = rows
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid rows"))?;
    PtySize::new(cols, rows)
}

fn parse_stty_size(value: &str) -> io::Result<PtySize> {
    let mut parts = value.split_whitespace();
    let rows = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing rows"))?
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid rows"))?;
    let cols = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing columns"))?
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid columns"))?;

    if parts.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unexpected extra size fields",
        ));
    }

    PtySize::new(cols, rows)
}

fn forward_stdin_until_detach(stream: &mut UnixStream) -> io::Result<()> {
    let mut stdin = io::stdin().lock();
    let mut buf = [0_u8; 1024];
    let mut saw_prefix = false;

    loop {
        let n = stdin.read(&mut buf)?;
        if n == 0 {
            break;
        }

        let mut out = Vec::with_capacity(n);
        for byte in &buf[..n] {
            if saw_prefix {
                if *byte == b'd' {
                    return Ok(());
                }
                out.push(0x02);
                saw_prefix = false;
            }

            if *byte == 0x02 {
                saw_prefix = true;
            } else {
                out.push(*byte);
            }
        }

        if !out.is_empty() {
            stream.write_all(&out)?;
        }
    }

    if saw_prefix {
        stream.write_all(&[0x02])?;
    }

    Ok(())
}

struct RawModeGuard {
    saved: Option<String>,
}

impl RawModeGuard {
    fn enable() -> Self {
        if !stdin_is_tty() {
            return Self { saved: None };
        }

        let saved = ProcessCommand::new("stty")
            .arg("-g")
            .stdin(Stdio::inherit())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty());

        if saved.is_some() {
            let _ = ProcessCommand::new("stty")
                .args(["raw", "-echo"])
                .stdin(Stdio::inherit())
                .status();
        }

        Self { saved }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Some(saved) = &self.saved {
            let _ = ProcessCommand::new("stty")
                .arg(saved)
                .stdin(Stdio::inherit())
                .status();
        }
    }
}

fn stdin_is_tty() -> bool {
    unsafe extern "C" {
        fn isatty(fd: i32) -> i32;
    }

    unsafe { isatty(0) == 1 }
}

fn read_line(stream: &mut UnixStream) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];

    loop {
        let n = stream.read(&mut byte)?;
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
    fn parses_attach_size_override() {
        assert_eq!(
            parse_attach_size("120x40").unwrap(),
            crate::pty::PtySize {
                cols: 120,
                rows: 40
            }
        );
        assert!(parse_attach_size("0x40").is_err());
        assert!(parse_attach_size("120").is_err());
    }

    #[test]
    fn parses_stty_size_output_as_rows_then_cols() {
        assert_eq!(
            parse_stty_size("40 120\n").unwrap(),
            crate::pty::PtySize {
                cols: 120,
                rows: 40
            }
        );
    }
}
