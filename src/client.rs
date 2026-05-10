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
    let output = std::thread::spawn(move || copy_attach_output(&mut output_stream));

    let mut last_size = initial_size;
    let copy_mode_socket = socket.to_path_buf();
    let copy_mode_session = session.to_string();
    forward_stdin_until_detach(
        &mut stream,
        || {
            if take_winch_pending() {
                maybe_emit_resize(detect_attach_size(), &mut last_size, &mut on_resize)?;
            }
            Ok(())
        },
        |initial_input| run_copy_mode(&copy_mode_socket, &copy_mode_session, initial_input),
    )?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = output.join();
    Ok(())
}

fn copy_attach_output(output_stream: &mut UnixStream) {
    let mut buf = [0_u8; 8192];
    loop {
        let n = match output_stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };

        let mut stdout = io::stdout().lock();
        if stdout.write_all(&buf[..n]).is_err() {
            break;
        }
        let _ = stdout.flush();
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum AttachInputAction {
    Forward(Vec<u8>),
    EnterCopyMode {
        forward: Vec<u8>,
        initial_input: Vec<u8>,
    },
    Detach,
}

fn translate_attach_input(input: &[u8], saw_prefix: &mut bool) -> AttachInputAction {
    let mut output = Vec::with_capacity(input.len());

    for (index, byte) in input.iter().enumerate() {
        if *saw_prefix {
            *saw_prefix = false;
            match *byte {
                b'd' => return AttachInputAction::Detach,
                b'[' => {
                    return AttachInputAction::EnterCopyMode {
                        forward: output,
                        initial_input: input[index + 1..].to_vec(),
                    };
                }
                0x02 => {
                    output.push(0x02);
                    *saw_prefix = true;
                    continue;
                }
                _ => output.push(0x02),
            }
        }

        if *byte == 0x02 {
            *saw_prefix = true;
        } else {
            output.push(*byte);
        }
    }

    AttachInputAction::Forward(output)
}

fn forward_stdin_until_detach<F, C>(
    stream: &mut UnixStream,
    mut tick: F,
    mut enter_copy_mode: C,
) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
    C: FnMut(&[u8]) -> io::Result<()>,
{
    let mut buf = [0_u8; 1024];
    let mut saw_prefix = false;

    loop {
        tick()?;
        let n = io::stdin().lock().read(&mut buf)?;
        if n == 0 {
            break;
        }

        match translate_attach_input(&buf[..n], &mut saw_prefix) {
            AttachInputAction::Forward(output) => {
                if !output.is_empty() {
                    stream.write_all(&output)?;
                }
            }
            AttachInputAction::EnterCopyMode {
                forward,
                initial_input,
            } => {
                if !forward.is_empty() {
                    stream.write_all(&forward)?;
                }
                enter_copy_mode(&initial_input)?;
            }
            AttachInputAction::Detach => return Ok(()),
        }
    }

    if saw_prefix {
        stream.write_all(&[0x02])?;
    }

    Ok(())
}

fn run_copy_mode(socket: &Path, session: &str, initial_input: &[u8]) -> io::Result<()> {
    let body = send_control_request(
        socket,
        &protocol::encode_copy_mode(session, protocol::CaptureMode::All, None),
    )?;
    let output = String::from_utf8_lossy(&body);
    let mut view = CopyModeView::from_numbered_output(&output)?;

    write_copy_mode_view(&view)?;
    if view.is_empty() {
        write_copy_mode_message("empty")?;
        return Ok(());
    }

    for byte in initial_input {
        if handle_copy_mode_byte(socket, session, &mut view, *byte)? {
            return Ok(());
        }
    }

    let mut stdin = io::stdin().lock();
    let mut byte = [0_u8; 1];
    loop {
        let n = stdin.read(&mut byte)?;
        if n == 0 {
            break;
        }

        if handle_copy_mode_byte(socket, session, &mut view, byte[0])? {
            break;
        }
    }

    Ok(())
}

fn handle_copy_mode_byte(
    socket: &Path,
    session: &str,
    view: &mut CopyModeView,
    byte: u8,
) -> io::Result<bool> {
    match view.apply_key(byte) {
        CopyModeAction::Redraw => {
            write_copy_mode_view(view)?;
            Ok(false)
        }
        CopyModeAction::CopyLine(line) => {
            let body = send_control_request(
                socket,
                &protocol::encode_save_buffer(
                    session,
                    None,
                    protocol::CaptureMode::All,
                    protocol::BufferSelection::LineRange {
                        start: line,
                        end: line,
                    },
                ),
            )?;
            let saved = String::from_utf8_lossy(&body);
            let saved = saved.trim_end();
            if saved.is_empty() {
                write_copy_mode_message("copied")?;
            } else {
                write_copy_mode_message(&format!("copied to {saved}"))?;
            }
            Ok(true)
        }
        CopyModeAction::Exit => {
            write_copy_mode_message("exit")?;
            Ok(true)
        }
        CopyModeAction::Ignore => Ok(false),
    }
}

fn write_copy_mode_view(view: &CopyModeView) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(view.render().as_bytes())?;
    stdout.flush()
}

fn write_copy_mode_message(message: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(format!("\r\n-- copy mode: {message} --\r\n").as_bytes())?;
    stdout.flush()
}

fn send_control_request(socket: &Path, line: &str) -> io::Result<Vec<u8>> {
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(line.as_bytes())?;

    let response = read_line(&mut stream)?;
    if let Some(message) = response.strip_prefix("ERR ") {
        return Err(io::Error::other(message.trim_end().to_string()));
    }
    if response != "OK\n" {
        return Err(io::Error::other(format!(
            "unexpected server response: {response:?}"
        )));
    }

    let mut body = Vec::new();
    stream.read_to_end(&mut body)?;
    Ok(body)
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum CopyModeAction {
    Redraw,
    CopyLine(usize),
    Exit,
    Ignore,
}

struct CopyModeLine {
    number: usize,
    text: String,
}

struct CopyModeView {
    lines: Vec<CopyModeLine>,
    cursor: usize,
}

impl CopyModeView {
    fn from_numbered_output(output: &str) -> io::Result<Self> {
        let mut lines = Vec::new();
        for line in output.lines() {
            let Some((number, text)) = line.split_once('\t') else {
                continue;
            };
            let number = number.parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid copy-mode line number")
            })?;
            lines.push(CopyModeLine {
                number,
                text: text.to_string(),
            });
        }

        Ok(Self { lines, cursor: 0 })
    }

    fn cursor_line_number(&self) -> Option<usize> {
        self.lines.get(self.cursor).map(|line| line.number)
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    fn apply_key(&mut self, byte: u8) -> CopyModeAction {
        match byte {
            b'j' | 0x0e => {
                if self.cursor + 1 < self.lines.len() {
                    self.cursor += 1;
                    CopyModeAction::Redraw
                } else {
                    CopyModeAction::Ignore
                }
            }
            b'k' | 0x10 => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    CopyModeAction::Redraw
                } else {
                    CopyModeAction::Ignore
                }
            }
            b'y' | b'\r' | b'\n' => self
                .cursor_line_number()
                .map(CopyModeAction::CopyLine)
                .unwrap_or(CopyModeAction::Ignore),
            b'q' | 0x1b => CopyModeAction::Exit,
            _ => CopyModeAction::Ignore,
        }
    }

    fn render(&self) -> String {
        let mut output = String::from("\r\n-- copy mode --\r\n");
        for (index, line) in self.lines.iter().enumerate() {
            if index == self.cursor {
                output.push('>');
            } else {
                output.push(' ');
            }
            output.push(' ');
            output.push_str(&line.number.to_string());
            output.push('\t');
            output.push_str(&line.text);
            output.push_str("\r\n");
        }
        output
    }
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

    #[test]
    fn copy_mode_view_moves_with_vi_and_emacs_keys() {
        let mut view = CopyModeView::from_numbered_output("1\tfirst\n2\tsecond\n").unwrap();

        assert_eq!(view.cursor_line_number(), Some(1));
        assert_eq!(view.apply_key(b'j'), CopyModeAction::Redraw);
        assert_eq!(view.cursor_line_number(), Some(2));
        assert_eq!(view.apply_key(0x10), CopyModeAction::Redraw);
        assert_eq!(view.cursor_line_number(), Some(1));
    }

    #[test]
    fn copy_mode_view_copies_current_line() {
        let mut view = CopyModeView::from_numbered_output("7\tselected\n").unwrap();

        assert_eq!(view.apply_key(b'y'), CopyModeAction::CopyLine(7));
    }

    #[test]
    fn copy_mode_view_exits_on_q_or_escape() {
        let mut view = CopyModeView::from_numbered_output("1\tfirst\n").unwrap();

        assert_eq!(view.apply_key(b'q'), CopyModeAction::Exit);
        assert_eq!(view.apply_key(0x1b), CopyModeAction::Exit);
    }

    #[test]
    fn attach_input_dispatches_copy_mode_prefix_without_forwarding_bytes() {
        let action = translate_attach_input(b"\x02[", &mut false);

        assert_eq!(
            action,
            AttachInputAction::EnterCopyMode {
                forward: Vec::new(),
                initial_input: Vec::new(),
            }
        );
    }

    #[test]
    fn attach_input_detaches_on_prefix_d_without_forwarding_bytes() {
        let action = translate_attach_input(b"\x02d", &mut false);

        assert_eq!(action, AttachInputAction::Detach);
    }

    #[test]
    fn attach_input_passes_coalesced_copy_mode_keys_as_initial_input() {
        let action = translate_attach_input(b"\x02[y", &mut false);

        assert_eq!(
            action,
            AttachInputAction::EnterCopyMode {
                forward: Vec::new(),
                initial_input: vec![b'y'],
            }
        );
    }
}
