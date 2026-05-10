use crate::protocol;
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
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = output.join();
    Ok(())
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
