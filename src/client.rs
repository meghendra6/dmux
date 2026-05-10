use crate::protocol;
use crate::pty::PtySize;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

static WINCH_PENDING: AtomicBool = AtomicBool::new(false);

pub fn attach<F>(
    socket: &Path,
    session: &str,
    initial_size: Option<PtySize>,
    mut on_resize: F,
) -> io::Result<()>
where
    F: FnMut(PtySize) -> io::Result<()>,
{
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
    install_winch_handler();
    let mut output_stream = stream.try_clone()?;
    let output = std::thread::spawn(move || {
        let mut stdout = io::stdout().lock();
        let _ = io::copy(&mut output_stream, &mut stdout);
        let _ = stdout.flush();
    });

    let mut last_size = initial_size;
    forward_stdin_until_detach(&mut stream, || {
        if take_winch_pending() {
            maybe_emit_resize(detect_attach_size(), &mut last_size, &mut on_resize)?;
        }
        Ok(())
    })?;
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

fn maybe_emit_resize<F>(
    current: Option<PtySize>,
    last: &mut Option<PtySize>,
    emit: F,
) -> io::Result<()>
where
    F: FnOnce(PtySize) -> io::Result<()>,
{
    let Some(size) = current else {
        return Ok(());
    };

    if *last == Some(size) {
        return Ok(());
    }

    emit(size)?;
    *last = Some(size);
    Ok(())
}

fn forward_stdin_until_detach<F>(stream: &mut UnixStream, mut tick: F) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
{
    let mut stdin = io::stdin().lock();
    let mut buf = [0_u8; 1024];
    let mut saw_prefix = false;

    loop {
        tick()?;
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

fn install_winch_handler() {
    const SIGWINCH: i32 = 28;
    unsafe {
        signal(SIGWINCH, handle_winch);
    }
}

fn take_winch_pending() -> bool {
    WINCH_PENDING.swap(false, Ordering::SeqCst)
}

extern "C" fn handle_winch(_: i32) {
    WINCH_PENDING.store(true, Ordering::SeqCst);
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

unsafe extern "C" {
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> extern "C" fn(i32);
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

    #[test]
    fn maybe_emit_resize_only_emits_changed_sizes() {
        let mut last = Some(crate::pty::PtySize { cols: 80, rows: 24 });
        let mut emitted = Vec::new();

        maybe_emit_resize(
            Some(crate::pty::PtySize { cols: 80, rows: 24 }),
            &mut last,
            |size| {
                emitted.push(size);
                Ok(())
            },
        )
        .unwrap();
        maybe_emit_resize(
            Some(crate::pty::PtySize {
                cols: 100,
                rows: 40,
            }),
            &mut last,
            |size| {
                emitted.push(size);
                Ok(())
            },
        )
        .unwrap();
        maybe_emit_resize(None, &mut last, |size| {
            emitted.push(size);
            Ok(())
        })
        .unwrap();

        assert_eq!(
            emitted,
            vec![crate::pty::PtySize {
                cols: 100,
                rows: 40
            }]
        );
        assert_eq!(
            last,
            Some(crate::pty::PtySize {
                cols: 100,
                rows: 40
            })
        );
    }

    #[test]
    fn winch_flag_can_be_taken_once() {
        WINCH_PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(take_winch_pending());
        assert!(!take_winch_pending());
    }
}
