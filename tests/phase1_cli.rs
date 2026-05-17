use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::raw::{c_char, c_int, c_ulong, c_void};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SIGWINCH: c_int = 28;

#[cfg(any(target_os = "linux", target_os = "android"))]
const TIOCSWINSZ: c_ulong = 0x5414;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
const TIOCSWINSZ: c_ulong = 0x80087467;

#[repr(C)]
struct TestWinSize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

unsafe extern "C" {
    fn openpty(
        amaster: *mut c_int,
        aslave: *mut c_int,
        name: *mut c_char,
        termp: *const c_void,
        winp: *const TestWinSize,
    ) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
}

const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;

#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NONBLOCK: c_int = 0o4000;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
const O_NONBLOCK: c_int = 0x0004;

fn dmux(socket: &std::path::Path, args: &[&str]) -> Output {
    dmux_with_timeout(socket, args, Duration::from_secs(5))
}

fn dmux_with_timeout(socket: &std::path::Path, args: &[&str], timeout: Duration) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
    command.env("DEVMUX_SOCKET", socket).args(args);
    output_with_timeout(command, "run dmux", timeout)
}

fn output_with_timeout(mut command: Command, context: &str, timeout: Duration) -> Output {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect(context);
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let stdout_reader = spawn_output_reader(
        child.stdout.take().expect("capture command stdout"),
        Arc::clone(&stdout),
    );
    let stderr_reader = spawn_output_reader(
        child.stderr.take().expect("capture command stderr"),
        Arc::clone(&stderr),
    );
    let deadline = std::time::Instant::now() + timeout;

    loop {
        if let Some(status) = child.try_wait().expect("poll command") {
            stdout_reader
                .join()
                .expect("join command stdout")
                .expect("read command stdout");
            stderr_reader
                .join()
                .expect("join command stderr")
                .expect("read command stderr");
            return Output {
                status,
                stdout: stdout.lock().expect("lock command stdout").clone(),
                stderr: stderr.lock().expect("lock command stderr").clone(),
            };
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let status = child.wait().expect("wait timed-out command");
            stdout_reader
                .join()
                .expect("join timed-out command stdout")
                .expect("read timed-out command stdout");
            stderr_reader
                .join()
                .expect("join timed-out command stderr")
                .expect("read timed-out command stderr");
            panic!(
                "{context}: command timed out after {timeout:?} with {status:?}\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&stdout.lock().expect("lock timed-out command stdout")),
                String::from_utf8_lossy(&stderr.lock().expect("lock timed-out command stderr"))
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn unique_socket(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::path::PathBuf::from("/tmp")
        .join(format!("dmux-{name}-{}-{nanos}.sock", std::process::id()))
}

fn unique_temp_file(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::path::PathBuf::from("/tmp").join(format!("dmux-{name}-{}-{nanos}.tmp", std::process::id()))
}

fn unique_project_artifact(name: &str, extension: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::current_dir()
        .expect("current dir")
        .join("target")
        .join("dmux-test-artifacts");
    std::fs::create_dir_all(&dir).expect("create test artifact dir");
    dir.join(format!(
        "dmux-{name}-{}-{nanos}.{extension}",
        std::process::id()
    ))
}

fn unique_project_socket(name: &str) -> std::path::PathBuf {
    unique_project_artifact(name, "sock")
}

fn unique_project_command_file(name: &str) -> std::path::PathBuf {
    unique_project_artifact(name, "dmux")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_contains_ordered_line(text: &str, first: &str, middle: &str, last: &str) {
    for line in text.lines() {
        let Some(first_index) = line.find(first) else {
            continue;
        };
        let Some(middle_offset) = line[first_index + first.len()..].find(middle) else {
            continue;
        };
        let middle_index = first_index + first.len() + middle_offset;
        if line[middle_index + middle.len()..].contains(last) {
            return;
        }
    }

    panic!("missing ordered line containing {first:?}, {middle:?}, {last:?} in {text:?}");
}

fn assert_vertical_layout(text: &str, top: &str, bottom: &str) {
    let lines = text.lines().collect::<Vec<_>>();
    let top_index = lines
        .iter()
        .position(|line| line.contains(top))
        .unwrap_or_else(|| panic!("missing top {top:?} in {text:?}"));
    let bottom_index = lines
        .iter()
        .position(|line| line.contains(bottom))
        .unwrap_or_else(|| panic!("missing bottom {bottom:?} in {text:?}"));

    assert!(top_index < bottom_index, "{text:?}");
    assert!(
        lines[top_index + 1..bottom_index]
            .iter()
            .any(|line| !line.is_empty() && line.chars().all(|ch| ch == '─')),
        "{text:?}"
    );
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .args(["-c", "command -v \"$1\" >/dev/null 2>&1", "sh", name])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn read_socket_line(stream: &mut UnixStream) -> String {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];

    loop {
        let n = stream.read(&mut byte).expect("read socket response");
        if n == 0 {
            break;
        }
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }

    String::from_utf8(bytes).expect("utf8 socket response")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestPaneRegion {
    pane: usize,
    row_start: usize,
    row_end: usize,
    col_start: usize,
    col_end: usize,
}

fn attach_layout_regions(socket: &std::path::Path, session: &str) -> Vec<TestPaneRegion> {
    let mut stream = UnixStream::connect(socket).expect("connect socket");
    stream
        .write_all(format!("ATTACH_LAYOUT_SNAPSHOT\t{session}\n").as_bytes())
        .expect("write layout snapshot request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    let mut body = String::new();
    stream.read_to_string(&mut body).expect("read layout body");
    parse_layout_regions(&body)
}

fn raw_layout_epoch(socket: &std::path::Path, session: &str) -> u64 {
    let mut stream = UnixStream::connect(socket).expect("connect socket");
    stream
        .write_all(format!("ATTACH_RAW_STATE\t{session}\n").as_bytes())
        .expect("write raw state request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    read_socket_line(&mut stream)
        .strip_prefix("RAW_LAYOUT_EPOCH\t")
        .and_then(|line| line.trim_end().parse::<u64>().ok())
        .expect("raw layout epoch")
}

fn parse_layout_regions(body: &str) -> Vec<TestPaneRegion> {
    body.lines()
        .filter_map(|line| {
            let parts = line.split('\t').collect::<Vec<_>>();
            match parts.as_slice() {
                ["REGION", pane, row_start, row_end, col_start, col_end] => Some(TestPaneRegion {
                    pane: pane.parse().expect("region pane"),
                    row_start: row_start.parse().expect("region row start"),
                    row_end: row_end.parse().expect("region row end"),
                    col_start: col_start.parse().expect("region col start"),
                    col_end: col_end.parse().expect("region col end"),
                }),
                _ => None,
            }
        })
        .collect()
}

fn try_read_socket_line(stream: &mut UnixStream) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    let mut byte = [0_u8; 1];

    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                bytes.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if bytes.is_empty() {
                    return Ok(None);
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }

    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

fn attach_events_stream(socket: &std::path::Path, session: &str) -> UnixStream {
    let mut stream = UnixStream::connect(socket).expect("connect event stream");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .expect("set event stream timeout");
    stream
        .write_all(format!("ATTACH_EVENTS\t{session}\n").as_bytes())
        .expect("write attach events request");
    stream
}

fn attach_render_stream(socket: &std::path::Path, session: &str) -> UnixStream {
    let mut stream = UnixStream::connect(socket).expect("connect render stream");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .expect("set render stream timeout");
    stream
        .write_all(format!("ATTACH_RENDER\t{session}\n").as_bytes())
        .expect("write attach render request");
    stream
}

fn read_attach_render_frame_body(stream: &mut UnixStream) -> Vec<u8> {
    let line = read_socket_line(stream);
    let Some(len) = line
        .strip_prefix("FRAME\t")
        .and_then(|line| line.strip_suffix('\n'))
    else {
        panic!("invalid render frame header: {line:?}");
    };
    let len = len.parse::<usize>().expect("parse render frame length");
    let mut body = vec![0_u8; len];
    stream
        .read_exact(&mut body)
        .expect("read attach render frame body");
    body
}

fn try_read_attach_render_frame_body(stream: &mut UnixStream) -> std::io::Result<Option<Vec<u8>>> {
    let Some(line) = try_read_socket_line(stream)? else {
        return Ok(None);
    };
    let Some(len) = line
        .strip_prefix("FRAME\t")
        .and_then(|line| line.strip_suffix('\n'))
    else {
        panic!("invalid render frame header: {line:?}");
    };
    let len = len.parse::<usize>().expect("parse render frame length");
    let mut body = vec![0_u8; len];
    stream.read_exact(&mut body)?;
    Ok(Some(body))
}

fn attach_render_output_from_frame_body(body: &[u8]) -> Vec<u8> {
    let marker = b"OUTPUT\t";
    let output_header_start = body
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("render frame output header");
    let len_start = output_header_start + marker.len();
    let len_end = body[len_start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map(|offset| len_start + offset)
        .expect("render frame output header newline");
    let len = std::str::from_utf8(&body[len_start..len_end])
        .expect("render frame output length utf8")
        .parse::<usize>()
        .expect("render frame output length");
    let output_start = len_end + 1;
    let output_end = output_start + len;
    assert!(
        output_end <= body.len(),
        "render frame output length exceeds body"
    );
    assert_eq!(output_end, body.len(), "extra render frame body bytes");
    body[output_start..output_end].to_vec()
}

fn read_attach_render_frame_body_until_contains(stream: &mut UnixStream, needle: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        last = String::from_utf8_lossy(&read_attach_render_frame_body(stream)).to_string();
        if last.contains(needle) {
            return last;
        }
    }

    panic!("render stream frame did not contain {needle:?}; last:\n{last:?}");
}

#[test]
fn run_sequence_executes_commands_in_order() {
    let socket = unique_project_socket("run-sequence");
    let session = format!("run-sequence-{}", std::process::id());
    let sequence = format!(
        "new -d -s {session} -- sh -c 'printf batch-base; sleep 30'; \
         split-window -t {session} -v -- sh -c 'printf batch-split; sleep 30'; \
         rename-window -t {session} automation"
    );

    let output = dmux(&socket, &["run", &sequence]);
    assert_success(&output);

    let panes = dmux(
        &socket,
        &["list-panes", "-t", &session, "-F", "#{pane.index}"],
    );
    assert_success(&panes);
    assert_eq!(String::from_utf8_lossy(&panes.stdout).lines().count(), 2);
    let windows = dmux(&socket, &["list-windows", "-t", &session]);
    assert_success(&windows);
    assert!(String::from_utf8_lossy(&windows.stdout).contains("automation"));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn source_file_executes_commands_and_reports_line_errors() {
    let socket = unique_project_socket("source-file");
    let session = format!("source-file-{}", std::process::id());
    let file = unique_project_command_file("source-file");
    std::fs::write(
        &file,
        format!(
            "# comments and blank lines are ignored\n\
             new -d -s {session} -- sh -c 'printf source-base; sleep 30'\n\
             \n\
             split-window -t {session} -v -- sh -c 'printf source-split; sleep 30'\n\
             rename-window -t {session} sourced\n"
        ),
    )
    .expect("write source file");

    let output = dmux(
        &socket,
        &["source-file", file.to_str().expect("source file path")],
    );
    assert_success(&output);
    let windows = dmux(&socket, &["list-windows", "-t", &session]);
    assert_success(&windows);
    assert!(String::from_utf8_lossy(&windows.stdout).contains("sourced"));

    let bad_file = unique_project_command_file("source-file-bad");
    std::fs::write(
        &bad_file,
        format!("rename-window -t {session} before-error\nsplit-window -t {session} -z\n"),
    )
    .expect("write bad source file");
    let failed = dmux(
        &socket,
        &[
            "source-file",
            bad_file.to_str().expect("bad source file path"),
        ],
    );
    assert!(!failed.status.success());
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(stderr.contains("line 2"), "{stderr}");
    assert!(stderr.contains("split-window"), "{stderr}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(bad_file);
}

#[test]
fn run_shell_reports_success_output_and_failure_status() {
    let socket = unique_project_socket("run-shell");

    let success = dmux(&socket, &["run-shell", "printf shell-ok"]);
    assert_success(&success);
    assert_eq!(String::from_utf8_lossy(&success.stdout), "shell-ok");

    let failure = dmux(&socket, &["run-shell", "exit 7"]);
    assert!(!failure.status.success());
    let stderr = String::from_utf8_lossy(&failure.stderr);
    assert!(
        stderr.contains("run-shell exited with status 7"),
        "{stderr}"
    );
}

#[test]
fn run_shell_bounds_large_output_while_child_is_running() {
    let socket = unique_project_socket("run-shell-large-output");
    let output = dmux(&socket, &["run-shell", "yes x | head -c 70000"]);

    assert_success(&output);
    assert_eq!(output.stdout.len(), 64 * 1024);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("run-shell output truncated after 65536 bytes"),
        "{stderr}"
    );
}

#[test]
fn run_and_source_file_preserve_run_shell_quoting() {
    let socket = unique_project_socket("run-shell-quoting");
    let sequence = "run-shell printf '%s\\n' 'hello world'";
    let output = dmux(&socket, &["run", sequence]);

    assert_success(&output);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello world\n");

    let file = unique_project_command_file("run-shell-quoting");
    std::fs::write(&file, format!("{sequence}\n")).expect("write command file");
    let sourced = dmux(
        &socket,
        &["source-file", file.to_str().expect("command file path")],
    );

    assert_success(&sourced);
    assert_eq!(String::from_utf8_lossy(&sourced.stdout), "hello world\n");
    let _ = std::fs::remove_file(file);
}

#[test]
fn run_shell_in_run_preserves_single_quoted_backslash() {
    let socket = unique_project_socket("run-shell-single-quote-backslash");
    let output = dmux(&socket, &["run", r"run-shell printf '%s\n' '\'"]);

    assert_success(&output);
    assert_eq!(String::from_utf8_lossy(&output.stdout), "\\\n");
}

#[test]
fn set_option_prefix_moves_and_unbinds_send_prefix_binding() {
    let socket = unique_socket("prefix-send-binding");
    let session = format!("prefix-send-binding-{}", std::process::id());

    assert_success(&dmux(&socket, &["new", "-d", "-s", &session]));

    let before = dmux(&socket, &["list-keys", "-F", "#{key}=#{command}"]);
    assert_success(&before);
    let before = String::from_utf8_lossy(&before.stdout);
    assert!(
        before.lines().any(|line| line == "C-b=send-prefix"),
        "{before}"
    );

    assert_success(&dmux(&socket, &["set-option", "prefix", "C-a"]));
    let after = dmux(&socket, &["list-keys", "-F", "#{key}=#{command}"]);
    assert_success(&after);
    let after = String::from_utf8_lossy(&after.stdout);
    assert!(
        after.lines().any(|line| line == "C-a=send-prefix"),
        "{after}"
    );
    assert!(
        !after.lines().any(|line| line == "C-b=send-prefix"),
        "{after}"
    );

    assert_success(&dmux(&socket, &["unbind-key", "C-a"]));
    let unbound = dmux(&socket, &["list-keys", "-F", "#{key}=#{command}"]);
    assert_success(&unbound);
    let unbound = String::from_utf8_lossy(&unbound.stdout);
    assert!(
        !unbound.lines().any(|line| line == "C-a=send-prefix"),
        "{unbound}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

fn read_attach_render_output_until_contains(stream: &mut UnixStream, needle: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        let body = read_attach_render_frame_body(stream);
        last = String::from_utf8_lossy(&attach_render_output_from_frame_body(&body)).to_string();
        if last.contains(needle) {
            return last;
        }
    }

    panic!("render stream output did not contain {needle:?}; last:\n{last:?}");
}

fn count_attach_render_frames_until_contains(
    stream: &mut UnixStream,
    needle: &str,
) -> (usize, String) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut count = 0;
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        count += 1;
        last = String::from_utf8_lossy(&read_attach_render_frame_body(stream)).to_string();
        if last.contains(needle) {
            return (count, last);
        }
    }

    panic!("render stream frame did not contain {needle:?}; last:\n{last:?}");
}

fn count_attach_render_output_frames_until_contains(
    stream: &mut UnixStream,
    needle: &str,
) -> (usize, String, String) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut count = 0;
    let mut last_body = String::new();
    let mut last_output = String::new();

    while std::time::Instant::now() < deadline {
        let Some(body) =
            try_read_attach_render_frame_body(stream).expect("read attach render frame body")
        else {
            continue;
        };
        count += 1;
        last_body = String::from_utf8_lossy(&body).to_string();
        last_output =
            String::from_utf8_lossy(&attach_render_output_from_frame_body(&body)).to_string();
        if last_output.contains(needle) {
            return (count, last_body, last_output);
        }
    }

    panic!(
        "render stream output did not contain {needle:?}; last body:\n{last_body:?}\nlast output:\n{last_output:?}"
    );
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

fn save_buffer_text(socket: &std::path::Path, session: &str, name: &str, text: &str) {
    let mut stream = UnixStream::connect(socket).expect("connect socket");
    stream
        .write_all(
            format!(
                "SAVE_BUFFER_TEXT\t{session}\t{}\t{}\n",
                encode_hex(name.as_bytes()),
                encode_hex(text.as_bytes())
            )
            .as_bytes(),
        )
        .expect("write save buffer text request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), format!("{name}\n"));
}

#[test]
fn detached_session_keeps_process_output_available_for_capture() {
    let socket = unique_socket("capture");
    let session = format!("capture-{}", std::process::id());

    let output = dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 1; printf done; sleep 30",
        ],
    );
    assert_success(&output);

    let captured = poll_capture(&socket, &session, "done");
    assert!(captured.contains("ready"), "{captured:?}");
    assert!(captured.contains("done"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn capture_pane_modes_separate_history_from_screen() {
    let socket = unique_socket("capture-modes");
    let session = format!("capture-modes-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "for i in $(seq 1 30); do echo line-$i; done; sleep 30",
        ],
    ));
    let all = poll_capture(&socket, &session, "line-30");
    assert!(has_line(&all, "line-1"), "{all:?}");
    assert!(has_line(&all, "line-30"), "{all:?}");

    let screen = dmux(&socket, &["capture-pane", "-t", &session, "-p", "--screen"]);
    assert_success(&screen);
    let screen = String::from_utf8_lossy(&screen.stdout);
    assert!(!has_line(&screen, "line-1"), "{screen:?}");
    assert!(has_line(&screen, "line-30"), "{screen:?}");

    let history = dmux(
        &socket,
        &["capture-pane", "-t", &session, "-p", "--history"],
    );
    assert_success(&history);
    let history = String::from_utf8_lossy(&history.stdout);
    assert!(has_line(&history, "line-1"), "{history:?}");
    assert!(!has_line(&history, "line-30"), "{history:?}");

    let all = dmux(&socket, &["capture-pane", "-t", &session, "-p", "--all"]);
    assert_success(&all);
    let all = String::from_utf8_lossy(&all.stdout);
    assert!(has_line(&all, "line-1"), "{all:?}");
    assert!(has_line(&all, "line-30"), "{all:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn detached_session_sets_color_capable_terminal_environment() {
    let socket = unique_socket("terminal-env");
    let session = format!("terminal-env-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf 'TERM=%s COLORTERM=%s\\n' \"$TERM\" \"$COLORTERM\"; sleep 30",
        ],
    ));
    let captured = poll_capture(&socket, &session, "TERM=xterm-256color");
    assert!(
        captured.contains("TERM=xterm-256color COLORTERM=truecolor"),
        "{captured:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn pane_primary_device_attributes_query_gets_local_reply() {
    if !command_exists("python3") {
        return;
    }

    let socket = unique_socket("primary-da-query");
    let session = format!("primary-da-query-{}", std::process::id());
    let script = concat!(
        "import os,select,termios,time,tty;",
        "saved=termios.tcgetattr(0);",
        "tty.setraw(0);",
        "os.write(1,b'probe-before\\x1b[c');",
        "r,_,_=select.select([0],[],[],0.25);",
        "reply=os.read(0,64) if r else b'';",
        "os.write(1,b'probe-reply:'+reply.hex().encode()+b'\\n');",
        "termios.tcsetattr(0,termios.TCSANOW,saved);",
        "time.sleep(30)",
    );

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "python3", "-c", script],
    ));
    let captured = poll_capture(&socket, &session, "probe-reply:");
    assert!(
        captured.contains("probe-reply:1b5b3f313b3263"),
        "{captured:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn buffers_save_capture_list_paste_and_delete() {
    let socket = unique_socket("buffers");
    let source = format!("buffer-source-{}", std::process::id());
    let sink = format!("buffer-sink-{}", std::process::id());
    let file = unique_temp_file("buffer-paste");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &source,
            "--",
            "sh",
            "-c",
            "printf buffer-alpha; printf '\\n'; sleep 30",
        ],
    ));
    let captured = poll_capture(&socket, &source, "buffer-alpha");
    assert!(captured.contains("buffer-alpha"), "{captured:?}");

    let saved = dmux(
        &socket,
        &["save-buffer", "-t", &source, "-b", "saved", "--screen"],
    );
    assert_success(&saved);
    assert_eq!(String::from_utf8_lossy(&saved.stdout), "saved\n");

    let listed = dmux(&socket, &["list-buffers"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed.lines().any(|line| line == "saved\t13\tbuffer-alpha"),
        "{listed:?}"
    );

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &sink,
            "--",
            "sh",
            "-c",
            &format!("cat > {}; sleep 30", file.display()),
        ],
    ));

    assert_success(&dmux(
        &socket,
        &["paste-buffer", "-t", &sink, "-b", "saved"],
    ));
    assert!(poll_file_contains(&file, "buffer-alpha"));

    assert_success(&dmux(&socket, &["delete-buffer", "-b", "saved"]));
    let listed = dmux(&socket, &["list-buffers"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        !listed.lines().any(|line| line.starts_with("saved\t")),
        "{listed:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &source]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &sink]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn paste_buffer_wraps_text_when_pane_enables_bracketed_paste() {
    let socket = unique_socket("buffer-bracketed-paste");
    let session = format!("buffer-bracketed-paste-{}", std::process::id());
    let file = unique_temp_file("buffer-bracketed-paste");
    let _ = std::fs::remove_file(&file);
    let command = format!(
        "printf '\\033[?2004hbracketed-ready\\n'; stty raw -echo; cat > {}; sleep 30",
        file.display()
    );

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", &command],
    ));
    let ready = poll_capture(&socket, &session, "bracketed-ready");
    assert!(ready.contains("bracketed-ready"), "{ready:?}");
    save_buffer_text(&socket, &session, "payload", "buffer-alpha");

    assert_success(&dmux(
        &socket,
        &["paste-buffer", "-t", &session, "-b", "payload"],
    ));
    assert!(poll_file_equals(&file, "\x1b[200~buffer-alpha\x1b[201~"));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(&file);
}

#[test]
fn save_buffer_text_stores_literal_text_and_lists_preview() {
    let socket = unique_socket("save-buffer-text");
    let session = format!("save-buffer-text-{}", std::process::id());
    let text = "alpha\tbeta\nsecond line";

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(
            format!(
                "SAVE_BUFFER_TEXT\t{session}\t{}\t{}\n",
                encode_hex(b"composed"),
                encode_hex(text.as_bytes())
            )
            .as_bytes(),
        )
        .expect("write save buffer text request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), "composed\n");

    let listed = dmux(&socket, &["list-buffers"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed
            .lines()
            .any(|line| line == "composed\t22\talpha beta"),
        "{listed:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn save_buffer_text_missing_session_returns_error() {
    let socket = unique_socket("save-buffer-text-missing");
    let session = format!("save-buffer-text-base-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(
            format!(
                "SAVE_BUFFER_TEXT\tmissing-session\t{}\t{}\n",
                encode_hex(b"composed"),
                encode_hex(b"selected")
            )
            .as_bytes(),
        )
        .expect("write save buffer text request");
    assert_eq!(read_socket_line(&mut stream), "ERR missing session\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn save_buffer_can_copy_line_range_and_search_match() {
    let socket = unique_socket("copy-selection");
    let source = format!("copy-source-{}", std::process::id());
    let sink = format!("copy-sink-{}", std::process::id());
    let file = unique_temp_file("copy-selection");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &source,
            "--",
            "sh",
            "-c",
            "printf first; printf '\\n'; printf keep-one; printf '\\n'; printf keep-two; printf '\\n'; printf last; printf '\\n'; sleep 30",
        ],
    ));
    let captured = poll_capture(&socket, &source, "last");
    assert!(captured.contains("keep-one"), "{captured:?}");

    assert_success(&dmux(
        &socket,
        &[
            "save-buffer",
            "-t",
            &source,
            "-b",
            "picked",
            "--screen",
            "--start-line",
            "2",
            "--end-line",
            "3",
        ],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "save-buffer",
            "-t",
            &source,
            "-b",
            "match",
            "--screen",
            "--search",
            "last",
        ],
    ));

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &sink,
            "--",
            "sh",
            "-c",
            &format!("cat > {}; sleep 30", file.display()),
        ],
    ));

    assert_success(&dmux(
        &socket,
        &["paste-buffer", "-t", &sink, "-b", "picked"],
    ));
    assert_success(&dmux(
        &socket,
        &["paste-buffer", "-t", &sink, "-b", "match"],
    ));
    assert!(poll_file_equals(&file, "keep-one\nkeep-two\nlast\n"));

    assert_success(&dmux(&socket, &["kill-session", "-t", &source]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &sink]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn capture_and_save_buffer_support_tail_ranges_and_search_match_indexes() {
    let socket = unique_socket("copy-tail-range-search-index");
    let source = format!("copy-tail-range-search-index-{}", std::process::id());
    let sink = format!("copy-tail-range-search-index-sink-{}", std::process::id());
    let file = unique_temp_file("copy-tail-range-search-index");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &source,
            "--",
            "sh",
            "-c",
            "printf first; printf '\\n'; printf needle-one; printf '\\n'; printf middle; printf '\\n'; printf needle-two; printf '\\n'; printf tail; printf '\\n'; sleep 30",
        ],
    ));
    let captured = poll_capture(&socket, &source, "tail");
    assert!(captured.contains("needle-two"), "{captured:?}");

    let tail = dmux(
        &socket,
        &[
            "capture-pane",
            "-t",
            &source,
            "-p",
            "--screen",
            "--start-line",
            "-2",
            "--end-line",
            "-1",
        ],
    );
    assert_success(&tail);
    assert_eq!(String::from_utf8_lossy(&tail.stdout), "needle-two\ntail\n");

    let from_second_to_tail = dmux(
        &socket,
        &[
            "capture-pane",
            "-t",
            &source,
            "-p",
            "--screen",
            "--start-line",
            "2",
            "--end-line",
            "-1",
        ],
    );
    assert_success(&from_second_to_tail);
    assert_eq!(
        String::from_utf8_lossy(&from_second_to_tail.stdout),
        "needle-one\nmiddle\nneedle-two\ntail\n"
    );

    let second_capture_match = dmux(
        &socket,
        &[
            "capture-pane",
            "-t",
            &source,
            "-p",
            "--screen",
            "--search",
            "needle",
            "--match",
            "2",
        ],
    );
    assert_success(&second_capture_match);
    assert_eq!(
        String::from_utf8_lossy(&second_capture_match.stdout),
        "needle-two\n"
    );

    assert_success(&dmux(
        &socket,
        &[
            "save-buffer",
            "-t",
            &source,
            "-b",
            "tail",
            "--screen",
            "--start-line",
            "-2",
            "--end-line",
            "-1",
        ],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "save-buffer",
            "-t",
            &source,
            "-b",
            "mixed-range",
            "--screen",
            "--start-line",
            "2",
            "--end-line",
            "-1",
        ],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "save-buffer",
            "-t",
            &source,
            "-b",
            "second-match",
            "--screen",
            "--search",
            "needle",
            "--match",
            "2",
        ],
    ));

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &sink,
            "--",
            "sh",
            "-c",
            &format!("cat > {}; sleep 30", file.display()),
        ],
    ));
    assert_success(&dmux(&socket, &["paste-buffer", "-t", &sink, "-b", "tail"]));
    assert_success(&dmux(
        &socket,
        &["paste-buffer", "-t", &sink, "-b", "second-match"],
    ));
    assert_success(&dmux(
        &socket,
        &["paste-buffer", "-t", &sink, "-b", "mixed-range"],
    ));
    assert!(poll_file_equals(
        &file,
        "needle-two\ntail\nneedle-two\nneedle-one\nmiddle\nneedle-two\ntail\n"
    ));

    assert_success(&dmux(&socket, &["kill-session", "-t", &source]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &sink]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn list_buffers_format_exposes_latest_metadata_and_replacement_order() {
    let socket = unique_socket("buffer-list-format");
    let session = format!("buffer-list-format-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));

    for (name, text) in [
        ("alpha", "one\n"),
        ("beta", "two\n"),
        ("alpha", "one replaced\n"),
    ] {
        let mut stream = UnixStream::connect(&socket).expect("connect socket");
        stream
            .write_all(
                format!(
                    "SAVE_BUFFER_TEXT\t{session}\t{}\t{}\n",
                    encode_hex(name.as_bytes()),
                    encode_hex(text.as_bytes())
                )
                .as_bytes(),
            )
            .expect("write save buffer text request");
        assert_eq!(read_socket_line(&mut stream), "OK\n");
        assert_eq!(read_socket_line(&mut stream), format!("{name}\n"));
    }

    let listed = dmux(
        &socket,
        &[
            "list-buffers",
            "--format",
            "#{buffer.index}:#{buffer.name}:#{buffer.bytes}:#{buffer.lines}:#{buffer.latest}:#{buffer.preview}",
        ],
    );
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert_eq!(listed, "0:beta:4:1:0:two\n1:alpha:13:1:1:one replaced\n");

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(
            format!(
                "SAVE_BUFFER_TEXT\t{session}\t{}\t{}\n",
                encode_hex(b"#{buffer.latest}"),
                encode_hex(b"#{buffer.bytes}\n")
            )
            .as_bytes(),
        )
        .expect("write token-like save buffer text request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), "#{buffer.latest}\n");

    let listed = dmux(
        &socket,
        &[
            "list-buffers",
            "--format",
            "#{buffer.index}:#{buffer.name}:#{buffer.latest}:#{buffer.preview}",
        ],
    );
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert_eq!(
        listed,
        "0:beta:0:two\n1:alpha:0:one replaced\n2:#{buffer.latest}:1:#{buffer.bytes}\n"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn paste_buffer_rejects_exited_panes_and_missing_buffers_explicitly() {
    let socket = unique_socket("paste-safe");
    let session = format!("paste-safe-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "printf done"],
    ));
    let _ = poll_capture(&socket, &session, "done");

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(
            format!(
                "SAVE_BUFFER_TEXT\t{session}\t{}\t{}\n",
                encode_hex(b"saved"),
                encode_hex(b"text")
            )
            .as_bytes(),
        )
        .expect("write save buffer text request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), "saved\n");

    let missing = dmux(&socket, &["paste-buffer", "-t", &session, "-b", "missing"]);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("missing buffer"),
        "stderr:\n{}",
        String::from_utf8_lossy(&missing.stderr)
    );

    let exited = dmux(&socket, &["paste-buffer", "-t", &session, "-b", "saved"]);
    assert!(!exited.status.success());
    assert!(
        String::from_utf8_lossy(&exited.stderr).contains("pane is not running"),
        "stderr:\n{}",
        String::from_utf8_lossy(&exited.stderr)
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn save_buffer_command_uses_current_active_pane_after_split() {
    let socket = unique_socket("save-buffer-active-pane-after-split");
    let session = format!("save-buffer-active-pane-after-split-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-save-source; printf '\\n'; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-save-source");
    assert!(base.contains("base-save-source"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-save-active; printf '\\n'; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-save-active");
    assert!(split.contains("split-save-active"), "{split:?}");

    assert_success(&dmux(
        &socket,
        &[
            "save-buffer",
            "-t",
            &session,
            "-b",
            "active",
            "--screen",
            "--start-line",
            "1",
            "--end-line",
            "1",
        ],
    ));
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    assert_success(&dmux(
        &socket,
        &[
            "save-buffer",
            "-t",
            &session,
            "-b",
            "base",
            "--screen",
            "--start-line",
            "1",
            "--end-line",
            "1",
        ],
    ));

    let listed = dmux(&socket, &["list-buffers"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed
            .lines()
            .any(|line| line.ends_with("\tsplit-save-active")),
        "{listed:?}"
    );
    assert!(
        listed
            .lines()
            .any(|line| line.ends_with("\tbase-save-source")),
        "{listed:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn select_pane_direction_moves_focus_using_nested_layout_geometry() {
    let socket = unique_socket("select-direction");
    let session = format!("select-direction-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf top-left; printf '\\n'; sleep 30",
        ],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf right; printf '\\n'; sleep 30",
        ],
    ));
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-v",
            "--",
            "sh",
            "-c",
            "printf bottom-left; printf '\\n'; sleep 30",
        ],
    ));

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-D"]));
    assert_eq!(active_pane_index_and_id(&socket, &session).0, 2);
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-U"]));
    assert_eq!(active_pane_index_and_id(&socket, &session).0, 0);
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-R"]));
    assert_eq!(active_pane_index_and_id(&socket, &session).0, 1);

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let failed = dmux(&socket, &["select-pane", "-t", &session, "-U"]);
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr).contains("missing adjacent pane"),
        "stderr:\n{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert_eq!(active_pane_index_and_id(&socket, &session).0, 0);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn select_pane_by_id_survives_index_reassignment() {
    let socket = unique_socket("select-pane-id");
    let session = format!("select-pane-id-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "sleep 30",
        ],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-v",
            "--",
            "sh",
            "-c",
            "sleep 30",
        ],
    ));
    let stable_id = pane_id_at_index(&socket, &session, 2);

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "1"]));
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    assert_success(&dmux(
        &socket,
        &[
            "select-pane",
            "-t",
            &session,
            "--pane-id",
            &stable_id.to_string(),
        ],
    ));

    assert_eq!(active_pane_index_and_id(&socket, &session), (1, stable_id));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn copy_mode_prints_numbered_lines_and_search_matches() {
    let socket = unique_socket("copy-mode");
    let session = format!("copy-mode-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf first; printf '\\n'; printf needle-one; printf '\\n'; printf last; printf '\\n'; printf needle-two; printf '\\n'; sleep 30",
        ],
    ));
    let captured = poll_capture(&socket, &session, "needle-two");
    assert!(captured.contains("needle-one"), "{captured:?}");

    let output = dmux(&socket, &["copy-mode", "-t", &session, "--screen"]);
    assert_success(&output);
    let output = String::from_utf8_lossy(&output.stdout);
    assert!(output.contains("1\tfirst\n"), "{output:?}");
    assert!(output.contains("4\tneedle-two\n"), "{output:?}");

    let output = dmux(
        &socket,
        &[
            "copy-mode",
            "-t",
            &session,
            "--screen",
            "--search",
            "needle",
        ],
    );
    assert_success(&output);
    let output = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output, "2\tneedle-one\n4\tneedle-two\n");

    let output = dmux(
        &socket,
        &[
            "copy-mode",
            "-t",
            &session,
            "--screen",
            "--search",
            "needle",
            "--match",
            "2",
        ],
    );
    assert_success(&output);
    let output = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output, "4\tneedle-two\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn copy_mode_reports_missing_session() {
    let socket = unique_socket("copy-mode-missing");
    let output = dmux(&socket, &["copy-mode", "-t", "missing"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing session"),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert_success(&dmux(&socket, &["kill-server"]));
}

fn has_line(text: &str, needle: &str) -> bool {
    text.lines().any(|line| line == needle)
}

fn poll_capture(socket: &std::path::Path, session: &str, needle: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        let output = dmux(socket, &["capture-pane", "-t", session, "-p"]);
        assert_success(&output);
        last = String::from_utf8_lossy(&output.stdout).to_string();
        if last.contains(needle) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    last
}

fn poll_capture_screen(socket: &std::path::Path, session: &str, needle: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        let output = dmux(socket, &["capture-pane", "-t", session, "-p", "--screen"]);
        assert_success(&output);
        last = String::from_utf8_lossy(&output.stdout).to_string();
        if last.contains(needle) {
            return last;
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    last
}

fn poll_list_panes_contains(
    socket: &std::path::Path,
    session: &str,
    format: &str,
    needle: &str,
) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        let output = dmux(socket, &["list-panes", "-t", session, "-F", format]);
        assert_success(&output);
        last = String::from_utf8_lossy(&output.stdout).to_string();
        if last.contains(needle) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    last
}

fn active_pane_index_and_id(socket: &std::path::Path, session: &str) -> (usize, usize) {
    let output = dmux(
        socket,
        &[
            "list-panes",
            "-t",
            session,
            "-F",
            "#{pane.index}:#{pane.id}:#{pane.active}",
        ],
    );
    assert_success(&output);
    let listed = String::from_utf8_lossy(&output.stdout);
    listed
        .lines()
        .find_map(|line| {
            let parts = line.split(':').collect::<Vec<_>>();
            match parts.as_slice() {
                [index, id, "1"] => Some((
                    index.parse::<usize>().expect("pane index"),
                    id.parse::<usize>().expect("pane id"),
                )),
                _ => None,
            }
        })
        .unwrap_or_else(|| panic!("missing active pane in {listed:?}"))
}

fn pane_id_at_index(socket: &std::path::Path, session: &str, target_index: usize) -> usize {
    let output = dmux(
        socket,
        &[
            "list-panes",
            "-t",
            session,
            "-F",
            "#{pane.index}:#{pane.id}",
        ],
    );
    assert_success(&output);
    let listed = String::from_utf8_lossy(&output.stdout);
    listed
        .lines()
        .find_map(|line| {
            let (index, id) = line.split_once(':')?;
            (index.parse::<usize>().ok()? == target_index).then(|| id.parse::<usize>().unwrap())
        })
        .unwrap_or_else(|| panic!("missing pane index {target_index} in {listed:?}"))
}

fn poll_list_sessions(socket: &std::path::Path, needle: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        let output = dmux(socket, &["ls"]);
        if output.status.success() {
            last = String::from_utf8_lossy(&output.stdout).to_string();
            if last.lines().any(|line| line == needle) {
                return last;
            }
        } else {
            last = format!(
                "status: {:?}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    last
}

fn poll_capture_eventually(socket: &std::path::Path, session: &str, needle: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        let output = dmux(socket, &["capture-pane", "-t", session, "-p"]);
        if output.status.success() {
            last = String::from_utf8_lossy(&output.stdout).to_string();
            if last.contains(needle) {
                return last;
            }
        } else {
            last = format!(
                "status: {:?}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    panic!("capture for {session:?} did not contain {needle:?}; last:\n{last}");
}

fn poll_active_pane(socket: &std::path::Path, session: &str, expected: usize) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last = String::new();
    let expected = format!("{expected}\t1");

    while std::time::Instant::now() < deadline {
        let output = dmux(
            socket,
            &[
                "list-panes",
                "-t",
                session,
                "-F",
                "#{pane.index}\t#{pane.active}",
            ],
        );
        assert_success(&output);
        last = String::from_utf8_lossy(&output.stdout).to_string();
        if last.lines().any(|line| line == expected) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    last
}

fn poll_pane_count(socket: &std::path::Path, session: &str, expected: usize) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        let output = dmux(socket, &["list-panes", "-t", session]);
        assert_success(&output);
        last = String::from_utf8_lossy(&output.stdout).to_string();
        if last.lines().count() == expected {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    last
}

fn poll_pane_format(socket: &std::path::Path, session: &str, format: &str, needle: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        let output = dmux(socket, &["list-panes", "-t", session, "-F", format]);
        assert_success(&output);
        last = String::from_utf8_lossy(&output.stdout).to_string();
        if last.contains(needle) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    last
}

fn poll_file_contains(path: &std::path::Path, needle: &str) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if std::fs::read_to_string(path).is_ok_and(|text| text.contains(needle)) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

fn poll_file_equals(path: &std::path::Path, expected: &str) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if std::fs::read_to_string(path).is_ok_and(|text| text == expected) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

fn poll_file_exists(path: &std::path::Path) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

fn process_exists(pid: &str) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn poll_process_gone(pid: &str) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if !process_exists(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

trait TestChildProcess {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> std::io::Result<()>;
    fn wait_with_output(self) -> std::io::Result<Output>;
}

impl TestChildProcess for Child {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }

    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }

    fn wait_with_output(self) -> std::io::Result<Output> {
        Child::wait_with_output(self)
    }
}

fn wait_for_child_exit<C: TestChildProcess>(mut child: C) -> Output {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if child.try_wait().expect("poll child").is_some() {
            return child.wait_with_output().expect("wait child output");
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            return child.wait_with_output().expect("wait killed child output");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn assert_child_exits_within<C: TestChildProcess>(mut child: C, context: &str) -> Output {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if child.try_wait().expect("poll child").is_some() {
            return child.wait_with_output().expect("wait child output");
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("wait killed child output");
            panic!(
                "{context}: child did not exit before timeout\nstatus: {:?}\nstdout:\n{}\nstderr:\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

struct CapturedDmuxChild {
    child: Child,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stdout_reader: Option<JoinHandle<std::io::Result<()>>>,
    stderr_reader: Option<JoinHandle<std::io::Result<()>>>,
}

impl CapturedDmuxChild {
    fn stdin_mut(&mut self, context: &str) -> &mut ChildStdin {
        self.child.stdin.as_mut().expect(context)
    }

    fn assert_running(&mut self, context: &str) {
        if let Some(status) = self.child.try_wait().expect("poll captured dmux child") {
            self.join_readers();
            panic!(
                "{context}: child exited unexpectedly with {status:?}\nstdout:\n{}\nstderr:\n{}",
                self.stdout_text(),
                self.stderr_text()
            );
        }
    }

    fn wait_for_stdout_contains_all(&mut self, needles: &[&str], context: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let stdout = self.stdout_text();
            if needles.iter().all(|needle| stdout.contains(needle)) {
                self.assert_running(context);
                return;
            }
            if let Some(status) = self.child.try_wait().expect("poll captured dmux child") {
                self.join_readers();
                panic!(
                    "{context}: child exited before stdout contained {needles:?} with {status:?}\nstdout:\n{}\nstderr:\n{}",
                    self.stdout_text(),
                    self.stderr_text()
                );
            }
            if std::time::Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                self.join_readers();
                panic!(
                    "{context}: stdout did not contain {needles:?} before timeout\nstdout:\n{}\nstderr:\n{}",
                    self.stdout_text(),
                    self.stderr_text()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn join_readers(&mut self) {
        if let Some(reader) = self.stdout_reader.take() {
            reader
                .join()
                .expect("join stdout reader")
                .expect("read child stdout");
        }
        if let Some(reader) = self.stderr_reader.take() {
            reader
                .join()
                .expect("join stderr reader")
                .expect("read child stderr");
        }
    }

    fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().expect("lock stdout")).to_string()
    }

    fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().expect("lock stderr")).to_string()
    }
}

impl TestChildProcess for CapturedDmuxChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    fn wait_with_output(mut self) -> std::io::Result<Output> {
        let status = self.child.wait()?;
        self.join_readers();
        Ok(Output {
            status,
            stdout: self.stdout.lock().expect("lock stdout").clone(),
            stderr: self.stderr.lock().expect("lock stderr").clone(),
        })
    }
}

struct PtyDmuxChild {
    child: Child,
    master: Option<File>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stdout_reader: Option<JoinHandle<std::io::Result<()>>>,
    stdout_reader_stop: Arc<AtomicBool>,
}

impl PtyDmuxChild {
    fn resize(&self, cols: u16, rows: u16) {
        let master = self.master.as_ref().expect("pty master is open");
        set_pty_size(master.as_raw_fd(), cols, rows);
        let result = unsafe { kill(self.child.id() as c_int, SIGWINCH) };
        assert_eq!(
            result,
            0,
            "send SIGWINCH: {}",
            std::io::Error::last_os_error()
        );
    }

    fn write_all(&mut self, input: &[u8]) {
        let master = self.master.as_mut().expect("pty master is open");
        master.write_all(input).expect("write pty input");
        master.flush().expect("flush pty input");
    }

    fn assert_running(&mut self, context: &str) {
        if let Some(status) = self.child.try_wait().expect("poll pty dmux child") {
            panic!(
                "{context}: child exited unexpectedly with {status:?}\nstdout:\n{}",
                self.stdout_text()
            );
        }
    }

    fn wait_for_stdout_contains_all(&mut self, needles: &[&str], context: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let stdout = self.stdout_text();
            if needles.iter().all(|needle| stdout.contains(needle)) {
                self.assert_running(context);
                return;
            }
            if let Some(status) = self.child.try_wait().expect("poll pty dmux child") {
                panic!(
                    "{context}: child exited before stdout contained {needles:?} with {status:?}\nstdout:\n{}",
                    self.stdout_text()
                );
            }
            if std::time::Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!(
                    "{context}: stdout did not contain {needles:?} before timeout\nstdout:\n{}",
                    self.stdout_text()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn close_master(&mut self) {
        let _ = self.master.take();
        let deadline = std::time::Instant::now() + Duration::from_millis(250);
        let mut last_len = self.stdout.lock().expect("lock pty stdout").len();
        let mut idle_since = std::time::Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let len = self.stdout.lock().expect("lock pty stdout").len();
            if len != last_len {
                last_len = len;
                idle_since = std::time::Instant::now();
            }
            if idle_since.elapsed() >= Duration::from_millis(30)
                || std::time::Instant::now() >= deadline
            {
                break;
            }
        }
        self.stdout_reader_stop.store(true, Ordering::SeqCst);
    }

    fn join_reader(&mut self) {
        if let Some(reader) = self.stdout_reader.take() {
            reader
                .join()
                .expect("join pty stdout reader")
                .expect("read pty stdout");
        }
    }

    fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().expect("lock pty stdout")).to_string()
    }

    fn clear_stdout(&mut self) {
        self.stdout.lock().expect("lock pty stdout").clear();
    }

    fn wait_for_stdout_idle(&mut self, idle_for: Duration, context: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut last_len = self.stdout.lock().expect("lock pty stdout").len();
        let mut idle_since = std::time::Instant::now();

        loop {
            self.assert_running(context);
            let len = self.stdout.lock().expect("lock pty stdout").len();
            if len != last_len {
                last_len = len;
                idle_since = std::time::Instant::now();
            }
            if idle_since.elapsed() >= idle_for {
                return;
            }
            if std::time::Instant::now() >= deadline {
                panic!(
                    "{context}: stdout did not become idle for {idle_for:?}\nstdout:\n{}",
                    self.stdout_text()
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl TestChildProcess for PtyDmuxChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    fn wait_with_output(mut self) -> std::io::Result<Output> {
        let status = self.child.wait()?;
        self.close_master();
        self.join_reader();
        Ok(Output {
            status,
            stdout: self.stdout.lock().expect("lock pty stdout").clone(),
            stderr: Vec::new(),
        })
    }
}

fn spawn_attached_dmux(
    socket: &std::path::Path,
    args: &[&str],
    readiness_needles: &[&str],
) -> CapturedDmuxChild {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", socket)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn attached dmux");
    let stdout_pipe = child.stdout.take().expect("capture attached stdout");
    let stderr_pipe = child.stderr.take().expect("capture attached stderr");
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let mut child = CapturedDmuxChild {
        child,
        stdout_reader: Some(spawn_output_reader(stdout_pipe, Arc::clone(&stdout))),
        stderr_reader: Some(spawn_output_reader(stderr_pipe, Arc::clone(&stderr))),
        stdout,
        stderr,
    };
    child.wait_for_stdout_contains_all(readiness_needles, "attach readiness");
    child
}

fn spawn_pty_attached_dmux(
    socket: &std::path::Path,
    args: &[&str],
    cols: u16,
    rows: u16,
    readiness_needles: &[&str],
) -> PtyDmuxChild {
    spawn_pty_attached_dmux_with_env(socket, args, cols, rows, readiness_needles, &[])
}

fn spawn_pty_attached_dmux_with_env(
    socket: &std::path::Path,
    args: &[&str],
    cols: u16,
    rows: u16,
    readiness_needles: &[&str],
    envs: &[(&str, &str)],
) -> PtyDmuxChild {
    let (master, slave) = open_test_pty(cols, rows);
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stdout_reader_stop = Arc::new(AtomicBool::new(false));
    let stdout_reader = spawn_pty_output_reader(
        master.try_clone().expect("clone pty master"),
        Arc::clone(&stdout),
        Arc::clone(&stdout_reader_stop),
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
    command.env("DEVMUX_SOCKET", socket).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let child = command
        .stdin(Stdio::from(
            slave.try_clone().expect("clone pty slave stdin"),
        ))
        .stdout(Stdio::from(
            slave.try_clone().expect("clone pty slave stdout"),
        ))
        .stderr(Stdio::from(slave))
        .spawn()
        .expect("spawn pty attached dmux");
    let mut child = PtyDmuxChild {
        child,
        master: Some(master),
        stdout,
        stdout_reader: Some(stdout_reader),
        stdout_reader_stop,
    };
    child.wait_for_stdout_contains_all(readiness_needles, "pty attach readiness");
    child
}

fn spawn_attached_to_session(
    socket: &std::path::Path,
    session: &str,
    readiness_needles: &[&str],
) -> CapturedDmuxChild {
    spawn_attached_dmux(socket, &["attach", "-t", session], readiness_needles)
}

fn spawn_output_reader<R>(
    mut reader: R,
    output: Arc<Mutex<Vec<u8>>>,
) -> JoinHandle<std::io::Result<()>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 1024];
        loop {
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                return Ok(());
            }
            output
                .lock()
                .expect("lock captured child output")
                .extend_from_slice(&buffer[..n]);
        }
    })
}

fn spawn_pty_output_reader(
    mut reader: File,
    output: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<std::io::Result<()>> {
    std::thread::spawn(move || {
        set_nonblocking(reader.as_raw_fd());
        let mut buffer = [0_u8; 1024];
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            match reader.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(n) => output
                    .lock()
                    .expect("lock captured pty output")
                    .extend_from_slice(&buffer[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) if error.raw_os_error() == Some(5) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    })
}

fn set_nonblocking(fd: c_int) {
    let flags = unsafe { fcntl(fd, F_GETFL) };
    assert_ne!(
        flags,
        -1,
        "get fd flags: {}",
        std::io::Error::last_os_error()
    );
    let result = unsafe { fcntl(fd, F_SETFL, flags | O_NONBLOCK) };
    assert_ne!(
        result,
        -1,
        "set fd nonblocking: {}",
        std::io::Error::last_os_error()
    );
}

fn open_test_pty(cols: u16, rows: u16) -> (File, File) {
    let mut master: c_int = -1;
    let mut slave: c_int = -1;
    let winsize = TestWinSize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe {
        openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &winsize,
        )
    };
    assert_eq!(result, 0, "openpty: {}", std::io::Error::last_os_error());

    unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) }
}

fn set_pty_size(fd: c_int, cols: u16, rows: u16) {
    let winsize = TestWinSize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe { ioctl(fd, TIOCSWINSZ, &winsize) };
    assert_eq!(
        result,
        0,
        "resize pty to {cols}x{rows}: {}",
        std::io::Error::last_os_error()
    );
}

#[test]
fn list_sessions_reports_created_session() {
    let socket = unique_socket("list");
    let session = format!("list-{}", std::process::id());

    let output = dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    );
    assert_success(&output);

    let output = dmux(&socket, &["ls"]);
    assert_success(&output);
    let sessions = String::from_utf8_lossy(&output.stdout);
    assert!(sessions.lines().any(|line| line == session), "{sessions:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn dmux_without_args_creates_default_and_detaches() {
    let socket = unique_socket("open-default");

    let mut child = spawn_attached_dmux(&socket, &[], &["default", "C-b ? help"]);

    let listed = poll_list_sessions(&socket, "default");
    assert!(listed.lines().any(|line| line == "default"), "{listed:?}");

    {
        let stdin = child.stdin_mut("bare dmux stdin");
        stdin.write_all(b"\x02d").expect("write detach");
        stdin.flush().expect("flush detach");
    }

    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", "default"]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn dmux_without_args_attaches_existing_default_without_duplicate_session() {
    let socket = unique_socket("open-existing-default");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            "default",
            "--",
            "sh",
            "-c",
            "printf existing-default; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, "default", "existing-default");
    assert!(ready.contains("existing-default"), "{ready:?}");

    let mut child = spawn_attached_dmux(&socket, &[], &["existing-default"]);

    {
        let stdin = child.stdin_mut("bare dmux stdin");
        stdin.write_all(b"\x02d").expect("write detach");
        stdin.flush().expect("flush detach");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("existing-default"), "{stdout:?}");

    let listed = dmux(&socket, &["ls"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert_eq!(listed.lines().filter(|line| *line == "default").count(), 1);

    assert_success(&dmux(&socket, &["kill-session", "-t", "default"]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn without_server_does_not_start_daemon_for_explicit_read_or_kill_commands() {
    for (name, args) in [
        ("ls", vec!["ls"]),
        ("attach", vec!["attach", "-t", "missing"]),
        ("kill-session", vec!["kill-session", "-t", "missing"]),
    ] {
        let socket = unique_socket(name);
        let output = dmux(&socket, &args);

        assert!(!output.status.success(), "{name} unexpectedly succeeded");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("no dmux server running; create a session with dmux new -s <name>"),
            "{name} stderr: {stderr:?}"
        );
        assert!(!socket.exists(), "{name} left socket behind at {socket:?}");
    }
}

#[test]
fn new_existing_session_reports_attach_hint() {
    let socket = unique_socket("duplicate-new");
    let session = format!("duplicate-new-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));

    let output = dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("session already exists"), "{stderr:?}");
    assert!(
        stderr.contains(&format!("dmux attach -t {session}")),
        "{stderr:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn cli_help_lists_attach_and_attach_help() {
    let socket = unique_socket("cli-help");

    let output = dmux(&socket, &["--help"]);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("attach [-t <name>]"), "{stdout:?}");
    assert!(stdout.contains("dmux help attach"), "{stdout:?}");

    let output = dmux(&socket, &["attach", "--help"]);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage: dmux attach [-t <name>]"),
        "{stdout:?}"
    );
    assert!(
        stdout.contains("If -t is omitted, attach targets default."),
        "{stdout:?}"
    );
    assert!(stdout.contains("C-b d"), "{stdout:?}");
    assert!(stdout.contains("C-b %"), "{stdout:?}");
    assert!(stdout.contains("C-b \""), "{stdout:?}");
    assert!(stdout.contains("C-b h/j/k/l"), "{stdout:?}");
    assert!(stdout.contains("C-b x"), "{stdout:?}");
    assert!(stdout.contains("C-b z"), "{stdout:?}");
    assert!(stdout.contains("C-b ?"), "{stdout:?}");
    assert!(stdout.contains("Session:"), "{stdout:?}");
    assert!(stdout.contains("Prompt examples:"), "{stdout:?}");
    assert!(stdout.contains("copy-mode:"), "{stdout:?}");
}

#[test]
fn interactive_new_attaches_and_detaches_created_session() {
    let socket = unique_socket("interactive-new-attach");
    let session = format!("interactive-new-attach-{}", std::process::id());

    let mut child = spawn_attached_dmux(
        &socket,
        &[
            "new",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf interactive-ready; sleep 30",
        ],
        &["interactive-ready"],
    );

    let ready = poll_capture_eventually(&socket, &session, "interactive-ready");
    assert!(ready.contains("interactive-ready"), "{ready:?}");

    {
        let stdin = child.stdin_mut("interactive new stdin");
        stdin.write_all(b"\x02d").expect("write detach");
        stdin.flush().expect("flush detach");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("interactive-ready"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn interactive_new_default_shell_splits_and_remains_usable_in_real_pty() {
    let socket = unique_socket("interactive-new-pty-split");
    let session = format!("interactive-new-pty-split-{}", std::process::id());

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.write_all(b"printf base-ready\\n\r");
    let base = poll_capture_eventually(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["base-ready", "|"], "pty split redraw");

    child.write_all(b"printf split-ready\\n\r");
    let split = poll_capture_eventually(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    child.write_all(b"\x02hprintf base-after-split\\n\r");
    let base = poll_capture_eventually(&socket, &session, "base-after-split");
    assert!(base.contains("base-after-split"), "{base:?}");
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn interactive_split_preserves_unsubmitted_input_in_original_pane() {
    let socket = unique_socket("interactive-split-preserves-pending-input");
    let session = format!(
        "interactive-split-preserves-pending-input-{}",
        std::process::id()
    );

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.write_all(b"echo base-preserved");
    child.wait_for_stdout_contains_all(&["echo base-preserved"], "pending input echo");
    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["│"], "pty split redraw");

    child.write_all(b"\x02h\r");
    let base = poll_capture_eventually(&socket, &session, "base-preserved");
    assert!(base.contains("base-preserved"), "{base:?}");
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn live_snapshot_attach_does_not_spam_idle_full_frame_redraws() {
    let socket = unique_socket("live-snapshot-idle-redraw");
    let session = format!("live-snapshot-idle-redraw-{}", std::process::id());

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["│"], "pty split redraw");
    child.wait_for_stdout_idle(Duration::from_millis(250), "settle split redraw");
    child.clear_stdout();

    std::thread::sleep(Duration::from_millis(1200));
    let idle_output = child.stdout_text();
    assert!(
        !idle_output.contains(&session),
        "idle attach emitted a repeated full frame:\n{idle_output:?}"
    );

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn live_snapshot_attach_repaints_pane_output_without_repeating_full_clear() {
    let socket = unique_socket("live-snapshot-pushed-redraw");
    let session = format!("live-snapshot-pushed-redraw-{}", std::process::id());

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["│"], "pty split redraw");
    child.wait_for_stdout_idle(Duration::from_millis(250), "settle split redraw");
    child.clear_stdout();

    child.write_all(b"printf pushed-frame\\n\r");
    let captured = poll_capture_eventually(&socket, &session, "pushed-frame");
    assert!(captured.contains("pushed-frame"), "{captured:?}");
    child.wait_for_stdout_contains_all(&["pushed-frame"], "pushed pane output redraw");
    child.wait_for_stdout_idle(Duration::from_millis(250), "settle pushed redraw");
    let pushed_redraw_output = child.stdout_text();

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));

    assert!(
        !pushed_redraw_output.contains("\x1b[2J"),
        "pushed redraw repeated a full-screen clear:\n{pushed_redraw_output:?}"
    );
    assert!(
        !pushed_redraw_output.contains(&session),
        "pushed redraw repeated the unchanged status line:\n{pushed_redraw_output:?}"
    );
}

#[test]
fn live_snapshot_attach_does_not_repaint_stale_frame_before_forwarded_input_echo() {
    let socket = unique_socket("live-snapshot-no-stale-input-redraw");
    let session = format!("live-snapshot-no-stale-input-redraw-{}", std::process::id());

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["│"], "pty split redraw");
    child.wait_for_stdout_idle(Duration::from_millis(250), "settle split redraw");
    child.clear_stdout();

    std::thread::sleep(Duration::from_millis(150));
    child.write_all(b"echo stable-input\r");
    let captured = poll_capture_eventually(&socket, &session, "stable-input");
    assert!(captured.contains("stable-input"), "{captured:?}");
    child.wait_for_stdout_contains_all(&["stable-input"], "forwarded input render");
    child.wait_for_stdout_idle(Duration::from_millis(250), "settle forwarded input render");
    let output = child.stdout_text();

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));

    let repaint_count = output.matches("\x1b[H").count();
    assert!(
        repaint_count <= 1,
        "forwarded input caused an extra stale repaint before PTY output:\n{output:?}"
    );
}

#[test]
fn live_snapshot_attach_renders_utf8_output_after_split() {
    let socket = unique_socket("live-snapshot-utf8-after-split");
    let session = format!("live-snapshot-utf8-after-split-{}", std::process::id());

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["│"], "pty split redraw");
    child.clear_stdout();

    child.write_all("printf '한글 ✓\\n'\r".as_bytes());
    let captured = poll_capture_eventually(&socket, &session, "한글 ✓");
    assert!(captured.contains("한글 ✓"), "{captured:?}");
    child.wait_for_stdout_contains_all(&["한글 ✓"], "utf8 output render after split");

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn live_snapshot_attach_applies_cursor_restore_output_after_split() {
    let socket = unique_socket("live-snapshot-cursor-restore");
    let session = format!("live-snapshot-cursor-restore-{}", std::process::id());

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["│"], "pty split redraw");
    child.clear_stdout();

    child.write_all(b"printf 'ab\\0337cd\\0338Z\\n'\r");
    let captured = poll_capture_eventually(&socket, &session, "abZd");
    assert!(captured.contains("abZd"), "{captured:?}");
    child.wait_for_stdout_contains_all(&["abZd"], "cursor restore output render after split");

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn live_snapshot_attach_uses_alternate_screen_and_restores_on_detach() {
    let socket = unique_socket("live-snapshot-alt-screen");
    let session = format!("live-snapshot-alt-screen-{}", std::process::id());

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.clear_stdout();
    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["\x1b[?1049h", "|"], "enter snapshot screen");

    child.write_all(b"\x02d");
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\x1b[?1049l"),
        "snapshot attach did not restore alternate screen:\n{stdout:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn live_snapshot_frame_output_fits_attach_pty_rows() {
    let socket = unique_socket("live-snapshot-frame-height");
    let session = format!("live-snapshot-frame-height-{}", std::process::id());

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.clear_stdout();
    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["│"], "snapshot frame");

    let stdout = child.stdout_text();
    let frame = stdout
        .rsplit_once("\x1b[H")
        .map(|(_, frame)| frame)
        .unwrap_or(stdout.as_str());
    let rendered_rows = frame.matches("\r\n").count() + usize::from(!frame.ends_with("\r\n"));
    assert!(
        rendered_rows <= 24,
        "snapshot frame rendered {rendered_rows} rows in a 24-row PTY:\n{frame:?}"
    );

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn live_snapshot_attach_repaints_after_resize_with_render_diff_cache() {
    let socket = unique_socket("live-snapshot-diff-cache-resize");
    let session = format!("live-snapshot-diff-cache-resize-{}", std::process::id());

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["new", "-s", &session],
        80,
        24,
        &[&session, "C-b ? help"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
    );

    child.write_all(b"\x02%");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    child.wait_for_stdout_contains_all(&["│"], "split redraw before resize");
    child.wait_for_stdout_idle(Duration::from_millis(250), "settle split redraw");
    child.clear_stdout();

    child.resize(100, 30);
    child.wait_for_stdout_contains_all(&[&session, "│"], "resize redraw with diff cache");

    child.clear_stdout();
    child.write_all(b"printf after-resize\\n\r");
    let captured = poll_capture_eventually(&socket, &session, "after-resize");
    assert!(captured.contains("after-resize"), "{captured:?}");
    child.wait_for_stdout_contains_all(&["after-resize"], "post-resize pane output");

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn session_lifecycle_can_attach_detach_kill_shutdown_and_recreate() {
    let socket = unique_socket("session-lifecycle-smoke");
    let session = format!("session-lifecycle-smoke-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf smoke-ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "smoke-ready");
    assert!(ready.contains("smoke-ready"), "{ready:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["smoke-ready"]);
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach");
        stdin.flush().expect("flush detach");
    }
    assert_success(&wait_for_child_exit(child));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    let listed = dmux(&socket, &["ls"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(!listed.lines().any(|line| line == session), "{listed:?}");

    assert_success(&dmux(&socket, &["kill-server"]));
    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf recreated-ready; sleep 30",
        ],
    ));
    let recreated = poll_capture(&socket, &session, "recreated-ready");
    assert!(recreated.contains("recreated-ready"), "{recreated:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn kill_server_removes_stale_socket_path() {
    let socket = unique_socket("stale-kill-server");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind stale socket");
    drop(listener);

    let output = dmux(&socket, &["kill-server"]);

    assert_success(&output);
    assert!(!socket.exists(), "stale socket path should be removed");
}

#[test]
fn kill_server_does_not_remove_non_socket_path() {
    let socket = unique_socket("regular-file-kill-server");
    std::fs::write(&socket, b"not a socket").expect("write regular file");

    let output = dmux(&socket, &["kill-server"]);

    assert!(!output.status.success());
    assert!(socket.exists(), "regular file path should not be removed");
    std::fs::remove_file(&socket).expect("remove regular file");
}

#[test]
fn attach_prefix_question_prints_help_and_keeps_attach_running() {
    let socket = unique_socket("attach-help");
    let session = format!("attach-help-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf help-ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "help-ready");
    assert!(ready.contains("help-ready"), "{ready:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["help-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02?").expect("write help");
        stdin.flush().expect("flush help");
    }
    child.wait_for_stdout_contains_all(&["C-b d detach", "split-window"], "attach help output");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach");
        stdin.flush().expect("flush detach");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("C-b d detach"), "{stdout:?}");
    assert!(stdout.contains("split-window"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_question_prints_multi_pane_help_and_keeps_attach_running() {
    let socket = unique_socket("attach-multi-help");
    let session = format!("attach-multi-help-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-help; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-help");
    assert!(base.contains("base-help"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-help; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-help");
    assert!(split.contains("split-help"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-help", "split-help"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02?").expect("write help");
        stdin.flush().expect("flush help");
    }
    child.wait_for_stdout_contains_all(
        &["C-b d detach", "C-b q pane numbers", "split-window"],
        "multi-pane attach help output",
    );

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach");
        stdin.flush().expect("flush detach");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("C-b d detach"), "{stdout:?}");
    assert!(stdout.contains("C-b q pane numbers"), "{stdout:?}");
    assert!(stdout.contains("split-window"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_reports_missing_session() {
    let socket = unique_socket("missing-attach");
    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", "present", "--", "sh", "-c", "sleep 30"],
    ));

    let output = dmux(&socket, &["attach", "-t", "missing"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing session"), "{stderr:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", "present"]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn capture_pane_strips_sgr_sequences() {
    let socket = unique_socket("sgr-capture");
    let session = format!("sgr-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf '\\033[31mred\\033[0m\\n'; sleep 30",
        ],
    ));

    let captured = poll_capture(&socket, &session, "red");
    assert!(captured.contains("red"), "{captured:?}");
    assert!(!captured.contains('\x1b'), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn capture_pane_applies_carriage_return_overwrite() {
    let socket = unique_socket("cr-capture");
    let session = format!("cr-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf 'hello\\rworld'; sleep 30",
        ],
    ));

    let captured = poll_capture(&socket, &session, "world");
    assert!(captured.contains("world"), "{captured:?}");
    assert!(!captured.contains("hello"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn resize_pane_updates_child_pty_size() {
    let socket = unique_socket("resize");
    let session = format!("resize-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "stty size; while true; do sleep 0.2; stty size; done",
        ],
    ));

    let initial = poll_capture(&socket, &session, "24 80");
    assert!(initial.contains("24 80"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "100", "-y", "40"],
    ));

    let resized = poll_capture(&socket, &session, "40 100");
    assert!(resized.contains("40 100"), "{resized:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_resizes_session_before_passthrough() {
    let socket = unique_socket("attach-size");
    let session = format!("attach-size-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "stty size; while true; do sleep 0.2; stty size; done",
        ],
    ));

    let initial = poll_capture(&socket, &session, "24 80");
    assert!(initial.contains("24 80"), "{initial:?}");

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .env("DEVMUX_ATTACH_SIZE", "132x43")
        .args(["attach", "-t", &session])
        .stdin(Stdio::null())
        .output()
        .expect("run attach");
    assert_success(&output);

    let resized = poll_capture(&socket, &session, "43 132");
    assert!(resized.contains("43 132"), "{resized:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn split_window_resizes_child_ptys_to_horizontal_layout_regions() {
    let socket = unique_socket("split-pty-size");
    let session = format!("split-pty-size-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "while true; do printf 'base:%s\\n' \"$(stty size)\"; sleep 0.1; done",
        ],
    ));
    let initial = poll_capture(&socket, &session, "base:24 80");
    assert!(initial.contains("base:24 80"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));
    let resized = poll_capture(&socket, &session, "base:24 83");
    assert!(resized.contains("base:24 83"), "{resized:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "while true; do printf 'split:%s\\n' \"$(stty size)\"; sleep 0.1; done",
        ],
    ));
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    let split = poll_capture(&socket, &session, "split:24 40");
    assert!(split.contains("split:24 40"), "{split:?}");

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let base = poll_capture(&socket, &session, "base:24 40");
    assert!(base.contains("base:24 40"), "{base:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn split_window_starts_child_pty_with_horizontal_layout_region_size() {
    let socket = unique_socket("split-initial-pty-size");
    let session = format!("split-initial-pty-size-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf 'initial-split-size:'; stty size; sleep 30",
        ],
    ));

    let split = poll_capture(&socket, &session, "initial-split-size:");
    assert!(split.contains("initial-split-size:24 40"), "{split:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn split_window_resizes_child_ptys_to_vertical_layout_regions() {
    let socket = unique_socket("split-vertical-pty-size");
    let session = format!("split-vertical-pty-size-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; while read line; do printf 'base-%s:%s\\n' \"$line\" \"$(stty size)\"; done",
        ],
    ));
    let initial = poll_capture(&socket, &session, "base-ready");
    assert!(initial.contains("base-ready"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "80", "-y", "25"],
    ));
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "before-split", "Enter"],
    ));
    let resized = poll_capture(&socket, &session, "base-before-split:25 80");
    assert!(resized.contains("base-before-split:25 80"), "{resized:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-v",
            "--",
            "sh",
            "-c",
            "printf split-ready; while read line; do printf 'split-%s:%s\\n' \"$line\" \"$(stty size)\"; done",
        ],
    ));
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-split", "Enter"],
    ));
    let split = poll_capture(&socket, &session, "split-after-split:12 80");
    assert!(split.contains("split-after-split:12 80"), "{split:?}");

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-select", "Enter"],
    ));
    let base = poll_capture(&socket, &session, "base-after-select:12 80");
    assert!(base.contains("base-after-select:12 80"), "{base:?}");

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "1"]));
    let panes = poll_pane_count(&socket, &session, 1);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0"]);
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-kill", "Enter"],
    ));
    let base = poll_capture(&socket, &session, "base-after-kill:25 80");
    assert!(base.contains("base-after-kill:25 80"), "{base:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn resize_pane_left_changes_horizontal_split_region_and_survives_absolute_resize() {
    let socket = unique_socket("resize-left-weighted");
    let session = format!("resize-left-weighted-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    assert!(poll_capture(&socket, &session, "base-ready").contains("base-ready"));
    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    assert!(poll_capture(&socket, &session, "split-ready").contains("split-ready"));

    let before = attach_layout_regions(&socket, &session);
    assert_eq!(before[0].col_end - before[0].col_start, 40);
    assert_eq!(before[1].col_end - before[1].col_start, 40);

    assert_success(&dmux(&socket, &["resize-pane", "-t", &session, "-L", "5"]));
    let after = attach_layout_regions(&socket, &session);
    assert_eq!(after[0].col_end - after[0].col_start, 35);
    assert_eq!(after[1].col_end - after[1].col_start, 45);

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "163", "-y", "24"],
    ));
    let resized = attach_layout_regions(&socket, &session);
    assert_eq!(resized[0].col_end - resized[0].col_start, 70);
    assert_eq!(resized[1].col_end - resized[1].col_start, 90);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn resize_pane_up_changes_vertical_split_region() {
    let socket = unique_socket("resize-up-weighted");
    let session = format!("resize-up-weighted-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    assert!(poll_capture(&socket, &session, "base-ready").contains("base-ready"));
    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "80", "-y", "25"],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-v",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    assert!(poll_capture(&socket, &session, "split-ready").contains("split-ready"));

    let before = attach_layout_regions(&socket, &session);
    assert_eq!(before[0].row_end - before[0].row_start, 12);
    assert_eq!(before[1].row_end - before[1].row_start, 12);

    assert_success(&dmux(&socket, &["resize-pane", "-t", &session, "-U", "5"]));
    let after = attach_layout_regions(&socket, &session);
    assert_eq!(after[0].row_end - after[0].row_start, 7);
    assert_eq!(after[1].row_end - after[1].row_start, 17);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn resize_pane_without_adjacent_boundary_errors_and_preserves_layout() {
    let socket = unique_socket("resize-missing-adjacent");
    let session = format!("resize-missing-adjacent-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    assert!(poll_capture(&socket, &session, "base-ready").contains("base-ready"));
    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    assert!(poll_capture(&socket, &session, "split-ready").contains("split-ready"));

    let before = attach_layout_regions(&socket, &session);
    let output = dmux(&socket, &["resize-pane", "-t", &session, "-R", "5"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing adjacent pane"),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(attach_layout_regions(&socket, &session), before);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn vertical_split_attach_preserves_recent_output_near_cursor() {
    let socket = unique_socket("vertical-split-preserve-recent");
    let session = format!("vertical-split-preserve-recent-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "for i in 01 02 03 04 05 06 07 08 09 10 11 12 13 14 15 16 17 18 19 20; do echo recent-$i; done; sleep 30",
        ],
    ));
    let before = poll_capture(&socket, &session, "recent-20");
    assert!(before.contains("recent-20"), "{before:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-v",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    let mut child = spawn_attached_to_session(&socket, &session, &["recent-20", "split-ready"]);
    child
        .stdin_mut("attach stdin")
        .write_all(b"\x02d")
        .expect("write detach");
    assert_success(&wait_for_child_exit(child));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_resize_updates_all_split_child_ptys() {
    let socket = unique_socket("attach-resize-split-ptys");
    let session = format!("attach-resize-split-ptys-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "while true; do printf 'base:%s\\n' \"$(stty size)\"; sleep 0.1; done",
        ],
    ));
    let initial = poll_capture(&socket, &session, "base:24 80");
    assert!(initial.contains("base:24 80"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "while true; do printf 'split:%s\\n' \"$(stty size)\"; sleep 0.1; done",
        ],
    ));
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .env("DEVMUX_ATTACH_SIZE", "83x24")
        .args(["attach", "-t", &session])
        .stdin(Stdio::null())
        .output()
        .expect("run attach");
    assert_success(&output);

    let split = poll_capture(&socket, &session, "split:24 40");
    assert!(split.contains("split:24 40"), "{split:?}");

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let base = poll_capture(&socket, &session, "base:24 40");
    assert!(base.contains("base:24 40"), "{base:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn snapshot_attach_forwards_live_terminal_resize_to_split_child_ptys() {
    let socket = unique_socket("snapshot-attach-live-resize");
    let session = format!("snapshot-attach-live-resize-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "while true; do printf 'base:%s\\n' \"$(stty size)\"; sleep 0.1; done",
        ],
    ));
    let base = poll_capture(&socket, &session, "base:24 80");
    assert!(base.contains("base:24 80"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "while true; do printf 'split:%s\\n' \"$(stty size)\"; sleep 0.1; done",
        ],
    ));
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    let mut child = spawn_pty_attached_dmux(
        &socket,
        &["attach", "-t", &session],
        80,
        24,
        &["base:", "split:"],
    );

    child.resize(100, 30);
    let split = poll_capture(&socket, &session, "split:30 49");
    assert!(split.contains("split:30 49"), "{split:?}");

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn kill_pane_resizes_remaining_child_pty_to_full_layout_region() {
    let socket = unique_socket("kill-resize-remaining-pty");
    let session = format!("kill-resize-remaining-pty-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; while read line; do printf 'base-%s:%s\\n' \"$line\" \"$(stty size)\"; done",
        ],
    ));
    let initial = poll_capture(&socket, &session, "base-ready");
    assert!(initial.contains("base-ready"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "before-split", "Enter"],
    ));
    let resized = poll_capture(&socket, &session, "base-before-split:24 83");
    assert!(resized.contains("base-before-split:24 83"), "{resized:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; while read line; do printf 'split-%s:%s\\n' \"$line\" \"$(stty size)\"; done",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-split", "Enter"],
    ));
    let split = poll_capture(&socket, &session, "split-after-split:24 40");
    assert!(split.contains("split-after-split:24 40"), "{split:?}");

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "1"]));
    let panes = poll_pane_count(&socket, &session, 1);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0"]);

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-kill", "Enter"],
    ));
    let base = poll_capture(&socket, &session, "base-after-kill:24 83");
    assert!(base.contains("base-after-kill:24 83"), "{base:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn exited_split_pane_stays_inspectable_and_active_moves_to_live_pane() {
    let socket = unique_socket("split-pane-exit-state");
    let session = format!("split-pane-exit-state-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; while read line; do printf 'base-%s:%s\\n' \"$line\" \"$(stty size)\"; done",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; exit 0",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "close", "Enter"],
    ));

    let panes = poll_list_panes_contains(
        &socket,
        &session,
        "#{pane.index}:#{pane.id}:#{pane.active}:#{pane.state}:#{pane.exit_status}",
        "1:1:0:exited:0",
    );
    assert_eq!(
        panes.lines().collect::<Vec<_>>(),
        vec!["0:0:1:running:", "1:1:0:exited:0"]
    );
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-exit", "Enter"],
    ));
    let base = poll_capture(&socket, &session, "base-after-exit:24 40");
    assert!(base.contains("base-after-exit:24 40"), "{base:?}");

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "1"]));
    let panes = poll_pane_count(&socket, &session, 1);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0"]);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn last_exited_pane_can_be_listed_captured_and_respawned_in_place() {
    let socket = unique_socket("last-exited-pane-respawn");
    let session = format!("last-exited-pane-respawn-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf before-exit; exit 7",
        ],
    ));

    let panes = poll_list_panes_contains(
        &socket,
        &session,
        "#{pane.index}:#{pane.id}:#{pane.active}:#{pane.state}:#{pane.exit_status}:#{pane.pid}",
        "0:0:1:exited:7:",
    );
    assert!(
        panes.lines().any(|line| {
            let parts = line.split(':').collect::<Vec<_>>();
            matches!(parts.as_slice(), ["0", "0", "1", "exited", "7", pid] if !pid.is_empty())
        }),
        "{panes:?}"
    );

    let captured = poll_capture(&socket, &session, "before-exit");
    assert!(captured.contains("before-exit"), "{captured:?}");

    let send = dmux(&socket, &["send-keys", "-t", &session, "ignored"]);
    assert!(!send.status.success(), "send-keys unexpectedly succeeded");
    assert!(
        String::from_utf8_lossy(&send.stderr).contains("pane is not running"),
        "{}",
        String::from_utf8_lossy(&send.stderr)
    );

    assert_success(&dmux(
        &socket,
        &[
            "respawn-pane",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf respawned-ready; sleep 30",
        ],
    ));
    let panes = poll_list_panes_contains(
        &socket,
        &session,
        "#{pane.index}:#{pane.id}:#{pane.active}:#{pane.state}:#{pane.exit_status}",
        "0:0:1:running:",
    );
    assert_eq!(panes.trim_end(), "0:0:1:running:");
    let captured = poll_capture(&socket, &session, "respawned-ready");
    assert!(captured.contains("respawned-ready"), "{captured:?}");

    assert_success(&dmux(
        &socket,
        &[
            "respawn-pane",
            "-k",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf fast-exit; exit 3",
        ],
    ));
    let panes = poll_list_panes_contains(
        &socket,
        &session,
        "#{pane.index}:#{pane.id}:#{pane.active}:#{pane.state}:#{pane.exit_status}",
        "0:0:1:exited:3",
    );
    assert_eq!(panes.trim_end(), "0:0:1:exited:3");
    let captured = poll_capture(&socket, &session, "fast-exit");
    assert!(captured.contains("fast-exit"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn respawn_running_pane_requires_force_and_preserves_pane_identity() {
    let socket = unique_socket("force-respawn-running-pane");
    let session = format!("force-respawn-running-pane-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf original-ready; sleep 30",
        ],
    ));
    let original = poll_capture(&socket, &session, "original-ready");
    assert!(original.contains("original-ready"), "{original:?}");

    let rejected = dmux(
        &socket,
        &[
            "respawn-pane",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf should-not-run; sleep 30",
        ],
    );
    assert!(!rejected.status.success(), "respawn unexpectedly succeeded");
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("pane is still running"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    assert_success(&dmux(
        &socket,
        &[
            "respawn-pane",
            "-k",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf forced-ready; sleep 30",
        ],
    ));
    let panes = poll_list_panes_contains(
        &socket,
        &session,
        "#{pane.index}:#{pane.id}:#{pane.active}:#{pane.state}",
        "0:0:1:running",
    );
    assert_eq!(panes.trim_end(), "0:0:1:running");
    let forced = poll_capture(&socket, &session, "forced-ready");
    assert!(forced.contains("forced-ready"), "{forced:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn zoom_pane_resizes_child_pty_between_full_and_split_layout_regions() {
    let socket = unique_socket("zoom-resize-pty");
    let session = format!("zoom-resize-pty-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; while read line; do printf 'base-%s:%s\\n' \"$line\" \"$(stty size)\"; done",
        ],
    ));
    let initial = poll_capture(&socket, &session, "base-ready");
    assert!(initial.contains("base-ready"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "before-split", "Enter"],
    ));
    let resized = poll_capture(&socket, &session, "base-before-split:24 83");
    assert!(resized.contains("base-before-split:24 83"), "{resized:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; while read line; do printf 'split-%s:%s\\n' \"$line\" \"$(stty size)\"; done",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-split", "Enter"],
    ));
    let split = poll_capture(&socket, &session, "split-after-split:24 40");
    assert!(split.contains("split-after-split:24 40"), "{split:?}");

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-zoom", "Enter"],
    ));
    let zoomed = poll_capture(&socket, &session, "split-after-zoom:24 83");
    assert!(zoomed.contains("split-after-zoom:24 83"), "{zoomed:?}");

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-unzoom", "Enter"],
    ));
    let unzoomed = poll_capture(&socket, &session, "split-after-unzoom:24 40");
    assert!(
        unzoomed.contains("split-after-unzoom:24 40"),
        "{unzoomed:?}"
    );

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-unzoom", "Enter"],
    ));
    let base = poll_capture(&socket, &session, "base-after-unzoom:24 40");
    assert!(base.contains("base-after-unzoom:24 40"), "{base:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn select_pane_while_zoomed_resizes_new_visible_child_pty_to_full_region() {
    let socket = unique_socket("zoom-select-resize-pty");
    let session = format!("zoom-select-resize-pty-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; while read line; do printf 'base-%s:%s\\n' \"$line\" \"$(stty size)\"; done",
        ],
    ));
    let initial = poll_capture(&socket, &session, "base-ready");
    assert!(initial.contains("base-ready"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; while read line; do printf 'split-%s:%s\\n' \"$line\" \"$(stty size)\"; done",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "after-zoom-select", "Enter"],
    ));
    let base = poll_capture(&socket, &session, "base-after-zoom-select:24 83");
    assert!(base.contains("base-after-zoom-select:24 83"), "{base:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_renders_status_line_snapshot() {
    let socket = unique_socket("attach-status-line");
    let session = format!("attach-status-line-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .args(["attach", "-t", &session])
        .stdin(Stdio::null())
        .output()
        .expect("run attach");
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("{session} [0] pane 0")),
        "{stdout:?}"
    );
    assert!(stdout.contains("prefix C-b"), "{stdout:?}");
    assert!(stdout.contains(": command"), "{stdout:?}");
    assert!(stdout.contains("[ copy"), "{stdout:?}");

    let captured = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert_success(&captured);
    let captured = String::from_utf8_lossy(&captured.stdout);
    assert!(
        !captured.contains(&format!("{session} [0] pane 0")),
        "{captured:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_renders_split_pane_snapshot() {
    let socket = unique_socket("attach-pane-snapshot");
    let session = format!("attach-pane-snapshot-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .args(["attach", "-t", &session])
        .stdin(Stdio::null())
        .output()
        .expect("run attach");
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_contains_ordered_line(&stdout, "base-ready", " │ ", "split-ready");
    assert!(!stdout.contains("-- pane 0 --"), "{stdout:?}");
    assert!(!stdout.contains("-- pane 1 --"), "{stdout:?}");

    let captured = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert_success(&captured);
    let captured = String::from_utf8_lossy(&captured.stdout);
    assert!(!captured.contains(" │ "), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_renders_styled_split_pane_frame() {
    let socket = unique_socket("attach-styled-pane-frame");
    let session = format!("attach-styled-pane-frame-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf '\\033[31mbase-styled\\033[0m'; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-styled");
    assert!(base.contains("base-styled"), "{base:?}");
    assert!(!base.contains('\x1b'), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf '\\033[1;38;2;1;2;3msplit-styled\\033[0m'; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-styled");
    assert!(split.contains("split-styled"), "{split:?}");
    assert!(!split.contains('\x1b'), "{split:?}");

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .env("DEVMUX_ATTACH_SIZE", "40x4")
        .args(["attach", "-t", &session])
        .stdin(Stdio::null())
        .output()
        .expect("run attach");
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\x1b[31mbase-styled\x1b[0m"), "{stdout:?}");
    assert!(
        stdout.contains("\x1b[1;38;2;1;2;3msplit-styled\x1b[0m"),
        "{stdout:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_renders_vertical_split_layout_snapshot() {
    let socket = unique_socket("attach-vertical-layout");
    let session = format!("attach-vertical-layout-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-v",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .args(["attach", "-t", &session])
        .stdin(Stdio::null())
        .output()
        .expect("run attach");
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_vertical_layout(&stdout, "base-ready", "split-ready");
    assert!(!stdout.contains("-- pane 0 --"), "{stdout:?}");
    assert!(!stdout.contains("-- pane 1 --"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_live_redraws_split_pane_output_after_attach_starts() {
    let socket = unique_socket("attach-live-redraw");
    let session = format!("attach-live-redraw-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo late:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "hello", "Enter"],
    ));
    let late = poll_capture(&socket, &session, "late:hello");
    assert!(late.contains("late:hello"), "{late:?}");
    child.wait_for_stdout_contains_all(&["late:hello"], "live attach redraw after pane output");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("base-ready"), "{stdout:?}");
    assert!(stdout.contains(" │ "), "{stdout:?}");
    assert!(stdout.contains("late:hello"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_live_input_routes_stdin_to_active_split_pane() {
    let socket = unique_socket("attach-live-input");
    let session = format!("attach-live-input-{}", std::process::id());
    let file = unique_temp_file("attach-live-input-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo typed:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"hello\r").expect("write attach input");
        stdin.flush().expect("flush attach input");
    }

    let typed = poll_capture(&socket, &session, "typed:hello");
    assert!(typed.contains("typed:hello"), "{typed:?}");

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"base-file\n")
            .expect("write selected pane attach input");
        stdin.flush().expect("flush selected pane attach input");
    }

    assert!(poll_file_contains(&file, "base-file"));

    child.wait_for_stdout_contains_all(&["typed:hello"], "live attach redraw after typed input");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("typed:hello"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn active_attach_exits_when_kill_session_runs_from_another_process() {
    let socket = unique_socket("active-attach-kill-session");
    let session = format!("active-attach-kill-session-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));

    let output = assert_child_exits_within(child, "raw attach after kill-session");
    assert_success(&output);
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn active_attach_exits_when_kill_server_runs_from_another_process() {
    let socket = unique_socket("active-attach-kill-server");
    let session = format!("active-attach-kill-server-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    assert_success(&dmux(&socket, &["kill-server"]));

    let output = assert_child_exits_within(child, "raw attach after kill-server");
    assert_success(&output);
}

#[test]
fn active_attach_redraws_when_pane_process_exits() {
    let socket = unique_socket("active-attach-pane-exit");
    let session = format!("active-attach-pane-exit-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 1",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    let panes = poll_list_panes_contains(
        &socket,
        &session,
        "#{pane.index}:#{pane.state}:#{pane.exit_status}",
        "0:exited:0",
    );
    assert_eq!(panes.trim_end(), "0:exited:0");
    child.wait_for_stdout_contains_all(&["pane 0"], "raw attach redraw after pane process exit");
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn active_multi_pane_attach_exits_when_kill_session_runs_from_another_process() {
    let socket = unique_socket("active-multi-attach-kill-session");
    let session = format!("active-multi-attach-kill-session-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));

    let output = assert_child_exits_within(child, "multi-pane attach after kill-session");
    assert_success(&output);
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_percent_splits_right_from_single_pane() {
    let socket = unique_socket("attach-prefix-percent");
    let session = format!("attach-prefix-percent-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02%").expect("write split right");
        stdin.flush().expect("flush split right");
    }

    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    let active = poll_active_pane(&socket, &session, 1);
    assert!(active.lines().any(|line| line == "1\t1"), "{active:?}");
    child.assert_running("attach after split");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("base-ready"), "{stdout:?}");
    assert!(stdout.contains(" │ "), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_percent_preserves_coalesced_input_after_raw_split() {
    let socket = unique_socket("attach-prefix-percent-coalesced");
    let session = format!("attach-prefix-percent-coalesced-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02%echo split-tail\r")
            .expect("write split and trailing input");
        stdin.flush().expect("flush split and trailing input");
    }

    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    let active = poll_active_pane(&socket, &session, 1);
    assert!(active.lines().any(|line| line == "1\t1"), "{active:?}");
    let captured = poll_capture(&socket, &session, "split-tail");
    assert!(captured.contains("split-tail"), "{captured:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_command_prompt_preserves_trailing_input_after_error() {
    let socket = unique_socket("attach-prompt-error-tail");
    let session = format!("attach-prompt-error-tail-{}", std::process::id());
    let file = unique_temp_file("attach-prompt-error-tail");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02:no-such-command\nraw-after-error\n")
            .expect("write failed command and trailing input");
        stdin
            .flush()
            .expect("flush failed command and trailing input");
    }

    assert!(poll_file_contains(&file, "raw-after-error"));
    child.assert_running("attach after failed prompt command");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(file);
}

#[test]
fn attach_prefix_percent_applies_coalesced_raw_focus_after_split() {
    let socket = unique_socket("attach-prefix-percent-raw-focus");
    let session = format!("attach-prefix-percent-raw-focus-{}", std::process::id());
    let base_file = unique_temp_file("attach-prefix-percent-raw-focus-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02%\x02hecho focus-tail\r")
            .expect("write split, raw focus, and trailing input");
        stdin.flush().expect("flush split input");
    }

    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    assert!(poll_file_contains(&base_file, "focus-tail"));
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn active_live_attach_exits_when_remaining_pane_process_exits_after_collapse() {
    let socket = unique_socket("active-live-attach-pane-exit-after-collapse");
    let session = format!(
        "active-live-attach-pane-exit-after-collapse-{}",
        std::process::id()
    );

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; read line; echo done:$line; sleep 1",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "1"]));
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"exit-after-collapse\r")
            .expect("write remaining pane input");
        stdin.flush().expect("flush remaining pane input");
    }
    let remaining = poll_capture(&socket, &session, "done:exit-after-collapse");
    assert!(
        remaining.contains("done:exit-after-collapse"),
        "{remaining:?}"
    );

    let output = assert_child_exits_within(
        child,
        "live attach after remaining pane process exits after collapse",
    );
    assert_success(&output);
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_percent_preserves_pending_prefix_for_detach_after_raw_split() {
    let socket = unique_socket("attach-prefix-percent-pending-prefix");
    let session = format!(
        "attach-prefix-percent-pending-prefix-{}",
        std::process::id()
    );

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02%\x02")
            .expect("write split and trailing prefix");
        stdin.flush().expect("flush split prefix");
    }
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"d").expect("write detach key");
        stdin.flush().expect("flush detach key");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_quote_splits_down_from_single_pane() {
    let socket = unique_socket("attach-prefix-quote");
    let session = format!("attach-prefix-quote-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02\"").expect("write split down");
        stdin.flush().expect("flush split down");
    }

    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    let active = poll_active_pane(&socket, &session, 1);
    assert!(active.lines().any(|line| line == "1\t1"), "{active:?}");
    child.assert_running("attach after split");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("base-ready"), "{stdout:?}");
    assert!(
        stdout
            .lines()
            .any(|line| !line.is_empty() && line.chars().all(|ch| ch == '─')),
        "{stdout:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_h_l_focuses_horizontal_panes_for_live_input() {
    let socket = unique_socket("attach-prefix-h-l");
    let session = format!("attach-prefix-h-l-{}", std::process::id());
    let base_file = unique_temp_file("attach-prefix-h-l-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-focus:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02hbase-left\n")
            .expect("write focus left and input");
        stdin.flush().expect("flush focus left");
    }
    assert!(poll_file_contains(&base_file, "base-left"));
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02lsplit-right\r")
            .expect("write focus right and input");
        stdin.flush().expect("flush focus right");
    }
    let split = poll_capture(&socket, &session, "split-focus:split-right");
    assert!(split.contains("split-focus:split-right"), "{split:?}");
    let active = poll_active_pane(&socket, &session, 1);
    assert!(active.lines().any(|line| line == "1\t1"), "{active:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn raw_attach_reconnects_when_external_split_creates_first_layout() {
    let socket = unique_socket("raw-attach-external-first-split");
    let session = format!("raw-attach-external-first-split-{}", std::process::id());
    let base_file = unique_temp_file("raw-attach-external-first-split-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut child = spawn_pty_attached_dmux(
        &socket,
        &["attach", "-t", &session],
        80,
        24,
        &["base-ready"],
    );
    std::thread::sleep(Duration::from_millis(150));

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf external-ready; read line; echo external:$line; sleep 30",
        ],
    ));
    child.wait_for_stdout_contains_all(&["external-ready"], "external first split transition");
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    child.write_all(b"\x02lok\r");
    let split = poll_capture(&socket, &session, "external:ok");
    assert!(split.contains("external:ok"), "{split:?}");
    let active = poll_active_pane(&socket, &session, 1);
    assert!(active.lines().any(|line| line == "1\t1"), "{active:?}");

    child.write_all(b"\x02d");
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_raw_state_epoch_marks_first_layout_transition() {
    let socket = unique_socket("attach-raw-state-epoch");
    let session = format!("attach-raw-state-epoch-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf epoch-base; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "epoch-base");
    assert!(base.contains("epoch-base"), "{base:?}");

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(format!("ATTACH_RAW_STATE\t{session}\n").as_bytes())
        .expect("write raw state request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), "RAW_LAYOUT_EPOCH\t0\n");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf epoch-split; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "epoch-split");
    assert!(split.contains("epoch-split"), "{split:?}");

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(format!("ATTACH_RAW_STATE\t{session}\n").as_bytes())
        .expect("write raw state request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), "RAW_LAYOUT_EPOCH\t1\n");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf epoch-third; sleep 30",
        ],
    ));
    let third = poll_capture(&socket, &session, "epoch-third");
    assert!(third.contains("epoch-third"), "{third:?}");

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(format!("ATTACH_RAW_STATE\t{session}\n").as_bytes())
        .expect("write raw state request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), "RAW_LAYOUT_EPOCH\t1\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn external_first_split_reconnects_multiple_raw_attach_clients() {
    let socket = unique_socket("raw-attach-external-first-split-multiple");
    let session = format!(
        "raw-attach-external-first-split-multiple-{}",
        std::process::id()
    );

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf multi-base; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "multi-base");
    assert!(base.contains("multi-base"), "{base:?}");

    let mut first = spawn_attached_to_session(&socket, &session, &["multi-base"]);
    let mut second = spawn_attached_to_session(&socket, &session, &["multi-base"]);

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf multi-split; sleep 30",
        ],
    ));
    first.wait_for_stdout_contains_all(&["multi-split"], "first raw attach transition");
    second.wait_for_stdout_contains_all(&["multi-split"], "second raw attach transition");

    {
        let stdin = first.stdin_mut("first attach stdin");
        stdin.write_all(b"\x02d").expect("write first detach");
        stdin.flush().expect("flush first detach");
    }
    {
        let stdin = second.stdin_mut("second attach stdin");
        stdin.write_all(b"\x02d").expect("write second detach");
        stdin.flush().expect("flush second detach");
    }
    assert_success(&wait_for_child_exit(first));
    assert_success(&wait_for_child_exit(second));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn raw_attach_copy_mode_waits_for_later_input() {
    let socket = unique_socket("raw-attach-copy-mode-later-input");
    let session = format!("raw-attach-copy-mode-later-input-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf raw-copy-later; printf '\\n'; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "raw-copy-later");
    assert!(base.contains("raw-copy-later"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["raw-copy-later"]);
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02[").expect("write copy-mode entry");
        stdin.flush().expect("flush copy-mode entry");
    }

    child.wait_for_stdout_contains_all(&["-- copy mode --"], "raw attach copy-mode entry");
    std::thread::sleep(std::time::Duration::from_millis(150));
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"y").expect("write delayed copy key");
        stdin.flush().expect("flush delayed copy key");
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut listed = String::new();
    while std::time::Instant::now() < deadline {
        let output = dmux(&socket, &["list-buffers"]);
        assert_success(&output);
        listed = String::from_utf8_lossy(&output.stdout).to_string();
        if listed
            .lines()
            .any(|line| line.ends_with("\t15\traw-copy-later"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        listed
            .lines()
            .any(|line| line.ends_with("\t15\traw-copy-later")),
        "{listed:?}"
    );

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn raw_attach_external_split_waits_for_copy_mode_to_finish() {
    let socket = unique_socket("raw-attach-external-split-copy-mode");
    let session = format!("raw-attach-external-split-copy-mode-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf copy-split-base; printf '\\n'; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "copy-split-base");
    assert!(base.contains("copy-split-base"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["copy-split-base"]);
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02[").expect("write copy-mode entry");
        stdin.flush().expect("flush copy-mode entry");
    }
    child.wait_for_stdout_contains_all(&["-- copy mode --"], "raw copy-mode before split");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf copy-split-ready; read line; echo copy-split:$line; sleep 30",
        ],
    ));
    std::thread::sleep(std::time::Duration::from_millis(150));

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"q").expect("write copy-mode exit key");
        stdin.flush().expect("flush copy-mode exit key");
    }

    child.wait_for_stdout_contains_all(
        &["copy-split-ready"],
        "external split transition after copy-mode",
    );
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02lok\n")
            .expect("write focus and split input");
        stdin.flush().expect("flush split input");
    }
    let split = poll_capture(&socket, &session, "copy-split:ok");
    assert!(split.contains("copy-split:ok"), "{split:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn raw_copy_mode_saves_view_text_after_external_active_pane_change() {
    let socket = unique_socket("raw-copy-mode-stable-source");
    let session = format!("raw-copy-mode-stable-source-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-copy-source; printf '\\n'; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-copy-source");
    assert!(base.contains("base-copy-source"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-copy-source"]);
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02[").expect("write copy-mode entry");
        stdin.flush().expect("flush copy-mode entry");
    }
    child.wait_for_stdout_contains_all(&["-- copy mode --"], "raw copy-mode before split");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-copy-source; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-copy-source");
    assert!(split.contains("split-copy-source"), "{split:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"y").expect("write copy key");
        stdin.flush().expect("flush copy key");
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut listed = String::new();
    while std::time::Instant::now() < deadline {
        let output = dmux(&socket, &["list-buffers"]);
        assert_success(&output);
        listed = String::from_utf8_lossy(&output.stdout).to_string();
        if listed
            .lines()
            .any(|line| line.ends_with("\tbase-copy-source"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        listed
            .lines()
            .any(|line| line.ends_with("\tbase-copy-source")),
        "{listed:?}"
    );
    assert!(
        !listed
            .lines()
            .any(|line| line.ends_with("\tsplit-copy-source")),
        "{listed:?}"
    );

    child.wait_for_stdout_contains_all(
        &["split-copy-source"],
        "external split transition after copy-mode save",
    );
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_l_uses_regions_from_external_split_render_frame() {
    let socket = unique_socket("attach-prefix-external-split-focus");
    let session = format!("attach-prefix-external-split-focus-{}", std::process::id());
    let base_file = unique_temp_file("attach-prefix-external-split-focus-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf initial-right; read line; echo initial-right:$line; sleep 30",
        ],
    ));
    let initial_right = poll_capture(&socket, &session, "initial-right");
    assert!(initial_right.contains("initial-right"), "{initial_right:?}");
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));

    let mut child = spawn_pty_attached_dmux(
        &socket,
        &["attach", "-t", &session],
        80,
        24,
        &["base-ready", "initial-right"],
    );
    std::thread::sleep(Duration::from_millis(150));

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf external-right; read line; echo external-right:$line; sleep 30",
        ],
    ));
    child.wait_for_stdout_contains_all(&["external-right"], "external split render frame");
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    {
        child.write_all(b"\x02lok\r");
    }

    let split = poll_capture(&socket, &session, "external-right:ok");
    assert!(split.contains("external-right:ok"), "{split:?}");
    let active = poll_active_pane(&socket, &session, 2);
    assert!(active.lines().any(|line| line == "2\t1"), "{active:?}");

    {
        child.write_all(b"\x02d");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_j_k_focuses_vertical_panes_for_live_input() {
    let socket = unique_socket("attach-prefix-j-k");
    let session = format!("attach-prefix-j-k-{}", std::process::id());
    let base_file = unique_temp_file("attach-prefix-j-k-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-v",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-focus:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02kbase-up\n")
            .expect("write focus up and input");
        stdin.flush().expect("flush focus up");
    }
    assert!(poll_file_contains(&base_file, "base-up"));
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02jsplit-down\r")
            .expect("write focus down and input");
        stdin.flush().expect("flush focus down");
    }
    let split = poll_capture(&socket, &session, "split-focus:split-down");
    assert!(split.contains("split-focus:split-down"), "{split:?}");
    let active = poll_active_pane(&socket, &session, 1);
    assert!(active.lines().any(|line| line == "1\t1"), "{active:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_x_closes_active_pane_and_reports_last_pane_error() {
    let socket = unique_socket("attach-prefix-x");
    let session = format!("attach-prefix-x-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; read line; echo base-after:$line; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02x").expect("write close pane");
        stdin.flush().expect("flush close pane");
    }
    let panes = poll_pane_count(&socket, &session, 1);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"visible\r\x02x\x02d")
            .expect("write remaining input close last and detach");
        stdin.flush().expect("flush remaining input");
    }
    let base = poll_capture(&socket, &session, "base-after:visible");
    assert!(base.contains("base-after:visible"), "{base:?}");

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cannot kill last pane; use kill-session"),
        "{stdout:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_z_toggles_zoom_for_active_pane() {
    let socket = unique_socket("attach-prefix-z");
    let session = format!("attach-prefix-z-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02z").expect("write zoom");
        stdin.flush().expect("flush zoom");
    }
    let panes = poll_pane_format(
        &socket,
        &session,
        "#{pane.index}:#{pane.active}:#{pane.zoomed}:#{window.zoomed_flag}",
        "1:1:1:1",
    );
    assert_eq!(
        panes.lines().collect::<Vec<_>>(),
        vec!["0:0:0:1", "1:1:1:1"]
    );

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02z").expect("write unzoom");
        stdin.flush().expect("flush unzoom");
    }
    let panes = poll_pane_format(
        &socket,
        &session,
        "#{pane.index}:#{pane.active}:#{pane.zoomed}:#{window.zoomed_flag}",
        "1:1:0:0",
    );
    assert_eq!(
        panes.lines().collect::<Vec<_>>(),
        vec!["0:0:0:0", "1:1:0:0"]
    );

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_z_reenters_snapshot_after_raw_unzoom() {
    let socket = unique_socket("attach-prefix-z-raw-unzoom");
    let session = format!("attach-prefix-z-raw-unzoom-{}", std::process::id());
    let base_file = unique_temp_file("attach-prefix-z-raw-unzoom-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-after:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");
    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));

    let mut child = spawn_attached_to_session(&socket, &session, &["split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02z\x02hbase-after\r")
            .expect("write unzoom, focus left, and input");
        stdin.flush().expect("flush unzoom input");
    }

    let panes = poll_pane_format(
        &socket,
        &session,
        "#{pane.index}:#{pane.active}:#{pane.zoomed}:#{window.zoomed_flag}",
        "0:1:0:0",
    );
    assert_eq!(
        panes.lines().collect::<Vec<_>>(),
        vec!["0:1:0:0", "1:0:0:0"]
    );
    assert!(poll_file_contains(&base_file, "base-after"));

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_detach_reattach_preserves_split_layout_and_active_input() {
    let socket = unique_socket("attach-reattach-layout");
    let session = format!("attach-reattach-layout-{}", std::process::id());
    let base_file = unique_temp_file("attach-reattach-layout-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02%").expect("write split command");
        stdin.flush().expect("flush split command");
    }
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02hbefore-detach\n\x02d")
            .expect("write first attach commands");
        stdin.flush().expect("flush first attach commands");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);
    assert!(poll_file_contains(&base_file, "before-detach"));
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", " │ "]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"after-reattach\n\x02d")
            .expect("write reattach input");
        stdin.flush().expect("flush reattach input");
    }
    assert!(poll_file_contains(&base_file, "after-reattach"));
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("base-ready"), "{stdout:?}");
    assert!(stdout.contains(" │ "), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_mouse_click_selects_pane_for_live_input() {
    let socket = unique_socket("attach-mouse-focus");
    let session = format!("attach-mouse-focus-{}", std::process::id());
    let base_file = unique_temp_file("attach-mouse-focus-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-mouse:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x1b[<0;1;2Mbase-mouse\n")
            .expect("write mouse click and base input");
        stdin.flush().expect("flush mouse input");
    }

    assert!(poll_file_contains(&base_file, "base-mouse"));
    let panes = poll_active_pane(&socket, &session, 0);
    assert!(panes.lines().any(|line| line == "0\t1"), "{panes:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_mouse_click_preserves_forwarded_input_before_focus_change() {
    let socket = unique_socket("attach-mouse-order");
    let session = format!("attach-mouse-order-{}", std::process::id());
    let base_file = unique_temp_file("attach-mouse-order-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-before:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"split\x0d\x1b[<0;1;2Mbase-after\n")
            .expect("write coalesced split input mouse click and base input");
        stdin.flush().expect("flush coalesced mouse input");
    }

    assert!(poll_file_contains(&base_file, "base-after"));
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "1"]));
    let split = poll_capture(&socket, &session, "split-before:split");
    assert!(split.contains("split-before:split"), "{split:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_mouse_click_on_separator_keeps_active_pane() {
    let socket = unique_socket("attach-mouse-separator");
    let session = format!("attach-mouse-separator-{}", std::process::id());
    let base_file = unique_temp_file("attach-mouse-separator-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-separator:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x1b[<0;1;2Mbase-before\n")
            .expect("write base click and input");
        stdin.flush().expect("flush base click input");
    }

    assert!(poll_file_contains(&base_file, "base-before"));
    let panes = poll_active_pane(&socket, &session, 0);
    assert!(panes.lines().any(|line| line == "0\t1"), "{panes:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x1b[<0;12;2Mbase-after\n")
            .expect("write separator click and split input");
        stdin.flush().expect("flush separator input");
    }

    let panes = poll_active_pane(&socket, &session, 0);
    assert!(panes.lines().any(|line| line == "0\t1"), "{panes:?}");
    assert!(poll_file_contains(&base_file, "base-after"));

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_mouse_click_in_blank_split_region_selects_pane() {
    let socket = unique_socket("attach-mouse-blank-region");
    let session = format!("attach-mouse-blank-region-{}", std::process::id());
    let base_file = unique_temp_file("attach-mouse-blank-region-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-blank:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x1b[<0;45;2Mblank-click\r")
            .expect("write blank-region click and split input");
        stdin.flush().expect("flush blank-region click input");
    }

    let split = poll_capture(&socket, &session, "split-blank:blank-click");
    assert!(split.contains("split-blank:blank-click"), "{split:?}");
    let panes = poll_active_pane(&socket, &session, 1);
    assert!(panes.lines().any(|line| line == "1\t1"), "{panes:?}");
    assert!(
        !poll_file_contains(&base_file, "blank-click"),
        "blank-region click should route input to the split pane"
    );

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_o_cycles_active_pane_for_live_input() {
    let socket = unique_socket("attach-pane-cycle");
    let session = format!("attach-pane-cycle-{}", std::process::id());
    let base_file = unique_temp_file("attach-pane-cycle-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-typed:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02obase-cycle\n")
            .expect("write coalesced cycle and base input");
        stdin.flush().expect("flush coalesced base input");
    }
    assert!(poll_file_contains(&base_file, "base-cycle"));
    let panes = poll_active_pane(&socket, &session, 0);
    assert!(panes.lines().any(|line| line == "0\t1"), "{panes:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02o").expect("write second cycle input");
        stdin.flush().expect("flush second cycle input");
    }
    let panes = poll_active_pane(&socket, &session, 1);
    assert!(panes.lines().any(|line| line == "1\t1"), "{panes:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"split\r").expect("write split input");
        stdin.flush().expect("flush split input");
    }
    let split = poll_capture(&socket, &session, "split-typed:split");
    assert!(split.contains("split-typed:split"), "{split:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_q_selects_numbered_pane_for_live_input() {
    let socket = unique_socket("attach-pane-number");
    let session = format!("attach-pane-number-{}", std::process::id());
    let base_file = unique_temp_file("attach-pane-number-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-number:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02q0base-number\n")
            .expect("write coalesced numbered selection and base input");
        stdin.flush().expect("flush numbered base input");
    }
    assert!(poll_file_contains(&base_file, "base-number"));
    let panes = poll_active_pane(&socket, &session, 0);
    assert!(panes.lines().any(|line| line == "0\t1"), "{panes:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02q1")
            .expect("write numbered split selection");
        stdin.flush().expect("flush numbered split selection");
    }
    let panes = poll_active_pane(&socket, &session, 1);
    assert!(panes.lines().any(|line| line == "1\t1"), "{panes:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"split\r").expect("write split input");
        stdin.flush().expect("flush split input");
    }
    let split = poll_capture(&socket, &session, "split-number:split");
    assert!(split.contains("split-number:split"), "{split:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("panes:"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_q_preserves_forwarded_input_before_number_selection() {
    let socket = unique_socket("attach-pane-number-order");
    let session = format!("attach-pane-number-order-{}", std::process::id());
    let base_file = unique_temp_file("attach-pane-number-order-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-before:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"split\x0d\x02q0base-after\n")
            .expect("write coalesced input before numbered selection");
        stdin.flush().expect("flush coalesced input");
    }

    assert!(poll_file_contains(&base_file, "base-after"));
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "1"]));
    let split = poll_capture(&socket, &session, "split-before:split");
    assert!(split.contains("split-before:split"), "{split:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_q_ignores_invalid_digit_and_keeps_attach_running() {
    let socket = unique_socket("attach-pane-number-invalid");
    let session = format!("attach-pane-number-invalid-{}", std::process::id());
    let base_file = unique_temp_file("attach-pane-number-invalid-base");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            &format!("printf base-ready; cat > {}; sleep 30", base_file.display()),
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo split-invalid:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02q9")
            .expect("write invalid pane selection");
        stdin.flush().expect("flush invalid pane selection");
    }
    let panes = poll_active_pane(&socket, &session, 1);
    assert!(panes.lines().any(|line| line == "1\t1"), "{panes:?}");
    child.assert_running("attach after invalid pane selection");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"still\r").expect("write split input");
        stdin.flush().expect("flush split input");
    }
    let split = poll_capture(&socket, &session, "split-invalid:still");
    assert!(split.contains("split-invalid:still"), "{split:?}");

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02q0base-after\n")
            .expect("write valid pane selection");
        stdin.flush().expect("flush valid pane selection");
    }
    assert!(poll_file_contains(&base_file, "base-after"));

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_bracket_copies_composed_layout_line_in_multi_pane_attach() {
    let socket = unique_socket("attach-copy-mode");
    let session = format!("attach-copy-mode-{}", std::process::id());
    let sink = format!("attach-copy-mode-sink-{}", std::process::id());
    let pasted_file = unique_temp_file("attach-copy-mode-paste");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-copy; printf '\\n'; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-copy");
    assert!(base.contains("base-copy"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-copy; printf '\\n'; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-copy");
    assert!(split.contains("split-copy"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-copy", "split-copy"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02[y")
            .expect("write copy-mode entry and copy key");
        stdin.flush().expect("flush copy-mode input");
    }

    let listed = poll_list_buffers_contains(&socket, "base-copy");
    assert!(
        listed
            .lines()
            .any(|line| line.contains("base-copy") && line.contains("│")),
        "{listed:?}"
    );

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &sink,
            "--",
            "sh",
            "-c",
            &format!("cat > {}; sleep 30", pasted_file.display()),
        ],
    ));
    assert_success(&dmux(&socket, &["paste-buffer", "-t", &sink]));
    assert!(poll_file_contains(&pasted_file, "base-copy"));
    assert!(poll_file_contains(&pasted_file, "split-copy"));

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("-- copy mode --"), "{stdout:?}");
    assert!(stdout.contains("-- copy mode: copied to "), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &sink]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(&pasted_file);
}

fn poll_list_buffers_contains(socket: &std::path::Path, needle: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut last = String::new();

    while std::time::Instant::now() < deadline {
        let output = dmux(socket, &["list-buffers"]);
        assert_success(&output);
        last = String::from_utf8_lossy(&output.stdout).to_string();
        if last.contains(needle) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    last
}

#[test]
fn attach_multi_pane_keeps_snapshot_handshake_for_client_compatibility() {
    let socket = unique_socket("attach-snapshot-handshake");
    let session = format!("attach-snapshot-handshake-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(format!("ATTACH\t{session}\n").as_bytes())
        .expect("write attach request");

    assert_eq!(read_socket_line(&mut stream), "OK\tSNAPSHOT\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_layout_snapshot_response_includes_regions_without_changing_plain_snapshot() {
    let socket = unique_socket("attach-layout-regions");
    let session = format!("attach-layout-regions-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut stream = UnixStream::connect(&socket).expect("connect socket");
    stream
        .write_all(format!("ATTACH_LAYOUT_SNAPSHOT\t{session}\n").as_bytes())
        .expect("write layout snapshot request");
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    let mut body = String::new();
    stream.read_to_string(&mut body).expect("read layout body");

    assert!(body.starts_with("REGIONS\t2\n"), "{body:?}");
    assert!(body.contains("REGION\t0\t0\t24\t0\t38\n"), "{body:?}");
    assert!(body.contains("REGION\t1\t0\t24\t41\t80\n"), "{body:?}");
    assert!(body.contains("SNAPSHOT\t"), "{body:?}");
    let composed_line = format!("base-ready{} │ split-ready", " ".repeat(28));
    assert!(body.contains(&composed_line), "{body:?}");

    let mut plain = UnixStream::connect(&socket).expect("connect socket");
    plain
        .write_all(format!("ATTACH_SNAPSHOT\t{session}\n").as_bytes())
        .expect("write plain snapshot request");
    assert_eq!(read_socket_line(&mut plain), "OK\n");
    let mut plain_body = String::new();
    plain
        .read_to_string(&mut plain_body)
        .expect("read plain snapshot body");
    assert!(!plain_body.contains("REGIONS\t"), "{plain_body:?}");
    assert!(plain_body.contains(&composed_line), "{plain_body:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_render_stream_sends_initial_terminal_output_frame() {
    let socket = unique_socket("attach-render-initial");
    let session = format!("attach-render-initial-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let body = read_attach_render_frame_body(&mut stream);
    let body_text = String::from_utf8_lossy(&body).to_string();
    let output = String::from_utf8_lossy(&attach_render_output_from_frame_body(&body)).to_string();

    assert!(
        body_text.starts_with("HEADER_ROWS\t1\nREGIONS\t"),
        "{body_text:?}"
    );
    assert!(body_text.contains("\nREGION\t"), "{body_text:?}");
    assert!(body_text.contains("\nOUTPUT\t"), "{body_text:?}");
    assert!(output.starts_with("\x1b[H"), "{output:?}");
    assert!(output.contains("\x1b[2K"), "{output:?}");
    assert!(output.contains("\x1b[?25h"), "{output:?}");
    assert!(!body_text.contains("STATUS\t"), "{body_text:?}");
    assert!(!body_text.contains("\nSNAPSHOT\t"), "{body_text:?}");
    assert!(output.contains(&session), "{output:?}");
    assert!(output.contains("base-ready"), "{output:?}");
    assert!(output.contains("split-ready"), "{output:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_render_stream_pushes_frame_after_pane_output() {
    let socket = unique_socket("attach-render-output");
    let session = format!("attach-render-output-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo late:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let initial = String::from_utf8_lossy(&read_attach_render_frame_body(&mut stream)).to_string();
    assert!(initial.contains("split-ready"), "{initial:?}");
    assert!(!initial.contains("STATUS\t"), "{initial:?}");
    assert!(!initial.contains("\nSNAPSHOT\t"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "hello", "Enter"],
    ));
    let late = poll_capture(&socket, &session, "late:hello");
    assert!(late.contains("late:hello"), "{late:?}");
    let pushed = read_attach_render_frame_body_until_contains(&mut stream, "late:hello");
    assert!(pushed.contains("late:hello"), "{pushed:?}");
    assert!(!pushed.contains("STATUS\t"), "{pushed:?}");
    assert!(!pushed.contains("\nSNAPSHOT\t"), "{pushed:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_render_stream_pushes_split_frame_after_structural_change() {
    let socket = unique_socket("attach-render-fast-split");
    let session = format!("attach-render-fast-split-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let initial = String::from_utf8_lossy(&read_attach_render_frame_body(&mut stream)).to_string();
    assert!(initial.contains("base-ready"), "{initial:?}");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set fast split render timeout");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));

    let pushed = String::from_utf8_lossy(&read_attach_render_frame_body(&mut stream)).to_string();
    assert!(pushed.contains("REGIONS\t2"), "{pushed:?}");
    assert!(pushed.contains("│"), "{pushed:?}");
    assert!(!pushed.contains("STATUS\t"), "{pushed:?}");
    assert!(!pushed.contains("\nSNAPSHOT\t"), "{pushed:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_render_stream_refreshes_scroll_region_redraw_after_split() {
    let socket = unique_socket("attach-render-scroll-region");
    let session = format!("attach-render-scroll-region-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let initial = String::from_utf8_lossy(&read_attach_render_frame_body(&mut stream)).to_string();
    assert!(initial.contains("base-ready"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf '\\033[?1049h\\033[H\\033[2Jheader\\r\\nview-a-01\\r\\nview-a-02\\r\\nview-a-03\\r\\nfooter\\033[2;4r\\033[2;1H\\033[3M\\033[2;1Hview-b-01\\r\\nview-b-02\\r\\nview-b-03\\033[6;1Hscroll-region-ready'; sleep 30",
        ],
    ));
    let captured = poll_capture(&socket, &session, "scroll-region-ready");
    assert!(captured.contains("view-b-03"), "{captured:?}");

    let pushed = read_attach_render_output_until_contains(&mut stream, "scroll-region-ready");
    assert!(pushed.contains("view-b-01"), "{pushed:?}");
    assert!(pushed.contains("view-b-02"), "{pushed:?}");
    assert!(pushed.contains("view-b-03"), "{pushed:?}");
    assert!(
        !pushed.contains("view-a-01")
            && !pushed.contains("view-a-02")
            && !pushed.contains("view-a-03"),
        "{pushed:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_render_stream_preserves_truecolor_background_after_line_erase() {
    let socket = unique_socket("attach-render-truecolor-bg");
    let session = format!("attach-render-truecolor-bg-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let initial = String::from_utf8_lossy(&read_attach_render_frame_body(&mut stream)).to_string();
    assert!(initial.contains("base-ready"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf '\\033[?1049h\\033[H\\033[2J\\033[38:2::1:2:3mfg-ready\\r\\n\\033[48:2::4:5:6m\\033[Kbg-ready'; sleep 30",
        ],
    ));

    let pushed = read_attach_render_output_until_contains(&mut stream, "bg-ready");
    assert!(pushed.contains("\x1b[38;2;1;2;3mfg-ready"), "{pushed:?}");
    assert!(pushed.contains("48;2;4;5;6m"), "{pushed:?}");
    assert!(pushed.contains("bg-ready"), "{pushed:?}");
    let bg_style_start = pushed
        .find("48;2;4;5;6m")
        .unwrap_or_else(|| panic!("missing background style in {pushed:?}"));
    let styled_tail = &pushed[bg_style_start..];
    let bg_reset = styled_tail
        .find("\x1b[0m")
        .unwrap_or_else(|| panic!("missing reset after background style in {pushed:?}"));
    assert!(
        styled_tail[..bg_reset].contains(&format!("bg-ready{}", " ".repeat(16))),
        "{pushed:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_render_stream_pushes_primary_output_promptly_after_alternate_screen_exit() {
    let socket = unique_socket("attach-render-alt-exit-primary");
    let session = format!("attach-render-alt-exit-primary-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let initial = String::from_utf8_lossy(&read_attach_render_frame_body(&mut stream)).to_string();
    assert!(initial.contains("base-ready"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf '\\033[?1049h\\033[?25l\\033[H\\033[2Jalt-title\\r\\nalt-body'; read line; printf '\\033[?1049l\\033[?25h'; read line; printf 'after-alt-exit-ready'; sleep 30",
        ],
    ));

    let alt = read_attach_render_output_until_contains(&mut stream, "alt-body");
    assert!(alt.contains("\x1b[?25l"), "{alt:?}");

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "exit-alt", "Enter"],
    ));
    let exit = read_attach_render_output_until_contains(&mut stream, "\x1b[?25h");
    assert!(!exit.contains("after-alt-exit-ready"), "{exit:?}");

    let started = std::time::Instant::now();
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "draw-primary", "Enter"],
    ));
    let (frames, frame_body, pushed) =
        count_attach_render_output_frames_until_contains(&mut stream, "after-alt-exit-ready");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "primary output after alternate-screen exit was delayed: {:?}",
        started.elapsed()
    );
    assert!(
        frames <= 3,
        "primary output after alternate-screen exit took too many frames: {frames}\n{frame_body:?}"
    );
    assert!(pushed.contains("after-alt-exit-ready"), "{pushed:?}");
    assert!(!pushed.contains("alt-title"), "{pushed:?}");
    assert!(!pushed.contains("alt-body"), "{pushed:?}");
    assert!(!frame_body.contains("STATUS\t"), "{frame_body:?}");
    assert!(!frame_body.contains("\nSNAPSHOT\t"), "{frame_body:?}");

    let captured = poll_capture(&socket, &session, "after-alt-exit-ready");
    assert!(captured.contains("after-alt-exit-ready"), "{captured:?}");
    assert!(!captured.contains("alt-title"), "{captured:?}");
    assert!(!captured.contains("alt-body"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_render_stream_returns_to_primary_screen_after_real_vim_exit() {
    assert!(
        command_exists("vim"),
        "real vim smoke test requires vim on PATH"
    );

    let socket = unique_socket("attach-render-real-vim-exit");
    let session = format!("attach-render-real-vim-exit-{}", std::process::id());
    let file = unique_temp_file("attach-render-real-vim-exit.md");
    std::fs::write(&file, "# dmux-vim-smoke-ready\nbody\n").expect("write vim smoke file");
    let file = file.to_string_lossy().to_string();
    let script = "printf 'primary-before-vim\\r\\n'; vim -Nu NONE -n -i NONE \"$1\"; printf 'after-real-vim-ready'; sleep 30";

    assert_success(&dmux(
        &socket,
        &[
            "new", "-d", "-s", &session, "--", "sh", "-c", script, "sh", &file,
        ],
    ));

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let vim_screen = read_attach_render_output_until_contains(&mut stream, "dmux-vim-smoke-ready");
    assert!(
        vim_screen.contains("dmux-vim-smoke-ready"),
        "{vim_screen:?}"
    );

    let started = std::time::Instant::now();
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "Escape", ":q!", "Enter"],
    ));
    let (frames, frame_body, output) =
        count_attach_render_output_frames_until_contains(&mut stream, "after-real-vim-ready");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "primary output after real vim exit was delayed: {:?}",
        started.elapsed()
    );
    assert!(
        frames <= 8,
        "primary output after real vim exit took too many frames: {frames}\n{frame_body:?}"
    );
    assert!(output.contains("primary-before-vim"), "{output:?}");
    assert!(output.contains("after-real-vim-ready"), "{output:?}");
    assert!(!output.contains("dmux-vim-smoke-ready"), "{output:?}");
    assert!(!frame_body.contains("STATUS\t"), "{frame_body:?}");
    assert!(!frame_body.contains("\nSNAPSHOT\t"), "{frame_body:?}");

    let captured = poll_capture_screen(&socket, &session, "after-real-vim-ready");
    assert!(captured.contains("primary-before-vim"), "{captured:?}");
    assert!(captured.contains("after-real-vim-ready"), "{captured:?}");
    assert!(!captured.contains("dmux-vim-smoke-ready"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(file);
}

#[test]
fn attach_render_stream_recovers_real_vim_after_split_closes() {
    assert!(
        command_exists("vim"),
        "real vim split/close test requires vim on PATH"
    );

    let socket = unique_socket("attach-render-real-vim-split-close");
    let session = format!("attach-render-real-vim-split-close-{}", std::process::id());
    let file = unique_temp_file("attach-render-real-vim-split-close.md");
    std::fs::write(&file, "# dmux-vim-split-close-ready\nbody\n")
        .expect("write vim split/close file");
    let file = file.to_string_lossy().to_string();
    struct DmuxTestCleanup {
        socket: std::path::PathBuf,
        session: String,
        file: String,
        kill_session: bool,
    }
    impl DmuxTestCleanup {
        fn disarm_session(&mut self) {
            self.kill_session = false;
        }
    }
    impl Drop for DmuxTestCleanup {
        fn drop(&mut self) {
            if self.kill_session {
                let _ = Command::new(env!("CARGO_BIN_EXE_dmux"))
                    .env("DEVMUX_SOCKET", &self.socket)
                    .arg("kill-session")
                    .arg("-t")
                    .arg(&self.session)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = Command::new(env!("CARGO_BIN_EXE_dmux"))
                    .env("DEVMUX_SOCKET", &self.socket)
                    .arg("kill-server")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = std::fs::remove_file(&self.file);
        }
    }
    let mut cleanup = DmuxTestCleanup {
        socket: socket.clone(),
        session: session.clone(),
        file: file.clone(),
        kill_session: true,
    };
    let script = "printf 'primary-before-vim-split-close\\r\\n'; vim -Nu NONE -n -i NONE \"$1\"; printf 'after-vim-split-close-ready'; sleep 30";

    assert_success(&dmux(
        &socket,
        &[
            "new", "-d", "-s", &session, "--", "sh", "-c", script, "sh", &file,
        ],
    ));

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let vim_screen =
        read_attach_render_output_until_contains(&mut stream, "dmux-vim-split-close-ready");
    assert!(
        vim_screen.contains("dmux-vim-split-close-ready"),
        "{vim_screen:?}"
    );

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-close-pane-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-close-pane-ready");
    assert!(split.contains("split-close-pane-ready"), "{split:?}");
    let panes = poll_pane_count(&socket, &session, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);
    let (_split_frames, split_frame_body, split_output) =
        count_attach_render_output_frames_until_contains(&mut stream, "split-close-pane-ready");
    assert!(
        split_output.contains("dmux-vim-split-close-ready"),
        "{split_output:?}"
    );
    assert!(
        !split_frame_body.contains("STATUS\t"),
        "{split_frame_body:?}"
    );
    assert!(
        !split_frame_body.contains("\nSNAPSHOT\t"),
        "{split_frame_body:?}"
    );

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "1"]));
    let panes = poll_pane_count(&socket, &session, 1);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0"]);
    let active = poll_active_pane(&socket, &session, 0);
    assert!(active.lines().any(|line| line == "0\t1"), "{active:?}");

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut last_body = String::new();
    let mut last_output = String::new();
    while std::time::Instant::now() < deadline {
        let Some(body) =
            try_read_attach_render_frame_body(&mut stream).expect("read attach render frame body")
        else {
            continue;
        };
        last_body = String::from_utf8_lossy(&body).to_string();
        last_output =
            String::from_utf8_lossy(&attach_render_output_from_frame_body(&body)).to_string();
        if last_output.contains("dmux-vim-split-close-ready")
            && !last_output.contains("split-close-pane-ready")
        {
            break;
        }
    }
    assert!(
        last_output.contains("dmux-vim-split-close-ready"),
        "{last_output:?}"
    );
    assert!(
        !last_output.contains("split-close-pane-ready"),
        "{last_output:?}"
    );
    assert!(!last_body.contains("STATUS\t"), "{last_body:?}");
    assert!(!last_body.contains("\nSNAPSHOT\t"), "{last_body:?}");

    let captured = poll_capture_screen(&socket, &session, "dmux-vim-split-close-ready");
    assert!(
        captured.contains("dmux-vim-split-close-ready"),
        "{captured:?}"
    );
    assert!(!captured.contains("split-close-pane-ready"), "{captured:?}");

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "Escape", ":q!", "Enter"],
    ));
    let (_frames, frame_body, output) = count_attach_render_output_frames_until_contains(
        &mut stream,
        "after-vim-split-close-ready",
    );
    assert!(
        output.contains("primary-before-vim-split-close"),
        "{output:?}"
    );
    assert!(output.contains("after-vim-split-close-ready"), "{output:?}");
    assert!(!output.contains("dmux-vim-split-close-ready"), "{output:?}");
    assert!(!output.contains("split-close-pane-ready"), "{output:?}");
    assert!(!frame_body.contains("STATUS\t"), "{frame_body:?}");
    assert!(!frame_body.contains("\nSNAPSHOT\t"), "{frame_body:?}");

    let captured = poll_capture_screen(&socket, &session, "after-vim-split-close-ready");
    assert!(
        captured.contains("primary-before-vim-split-close"),
        "{captured:?}"
    );
    assert!(
        captured.contains("after-vim-split-close-ready"),
        "{captured:?}"
    );
    assert!(
        !captured.contains("dmux-vim-split-close-ready"),
        "{captured:?}"
    );
    assert!(!captured.contains("split-close-pane-ready"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    cleanup.disarm_session();
}

#[test]
fn attach_render_stream_preserves_streaming_output_across_resize() {
    let socket = unique_socket("attach-render-stream-resize");
    let session = format!("attach-render-stream-resize-{}", std::process::id());
    let stop_file = unique_temp_file("attach-render-stream-resize-stop");
    let _ = std::fs::remove_file(&stop_file);
    let stop_file_arg = stop_file.to_string_lossy().into_owned();

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "stop_file=$1; printf stream-ready; read line; i=0; while [ ! -e \"$stop_file\" ] && [ $i -lt 200 ]; do i=$((i + 1)); printf 'stream-%03d\\n' \"$i\"; sleep 0.03; done; if [ -e \"$stop_file\" ]; then printf 'stream-done\\n'; else printf 'stream-aborted\\n'; fi; sleep 30",
            "sh",
            &stop_file_arg,
        ],
    ));
    let stream_ready = poll_capture(&socket, &session, "stream-ready");
    assert!(stream_ready.contains("stream-ready"), "{stream_ready:?}");

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let initial = String::from_utf8_lossy(&read_attach_render_frame_body(&mut stream)).to_string();
    assert!(initial.contains("base-ready"), "{initial:?}");
    assert!(initial.contains("stream-ready"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "go", "Enter"],
    ));
    let (_start_frames, start_frame_body, start_output) =
        count_attach_render_output_frames_until_contains(&mut stream, "stream-005");
    assert!(start_output.contains("stream-005"), "{start_output:?}");
    assert!(
        !start_frame_body.contains("STATUS\t"),
        "{start_frame_body:?}"
    );
    assert!(
        !start_frame_body.contains("\nSNAPSHOT\t"),
        "{start_frame_body:?}"
    );

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "100", "-y", "30"],
    ));
    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "80", "-y", "24"],
    ));
    let (_after_resize_frames, after_resize_frame_body, after_resize_output) =
        count_attach_render_output_frames_until_contains(&mut stream, "stream-010");
    assert!(
        after_resize_output.contains("stream-010"),
        "{after_resize_output:?}"
    );
    assert!(
        !after_resize_frame_body.contains("STATUS\t"),
        "{after_resize_frame_body:?}"
    );
    assert!(
        !after_resize_frame_body.contains("\nSNAPSHOT\t"),
        "{after_resize_frame_body:?}"
    );

    drop(File::create(&stop_file).expect("create stream stop file"));
    let (_frames, frame_body, output) =
        count_attach_render_output_frames_until_contains(&mut stream, "stream-done");
    assert!(output.contains("stream-done"), "{output:?}");
    assert!(!frame_body.contains("STATUS\t"), "{frame_body:?}");
    assert!(!frame_body.contains("\nSNAPSHOT\t"), "{frame_body:?}");

    let captured = poll_capture(&socket, &session, "stream-done");
    for i in 1..=10 {
        let line = format!("stream-{i:03}");
        assert!(captured.contains(&line), "missing {line} in {captured:?}");
    }
    assert!(captured.contains("stream-done"), "{captured:?}");
    assert!(!captured.contains('\x1b'), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(&stop_file);
}

#[test]
fn attach_render_stream_coalesces_bursty_pane_output() {
    let socket = unique_socket("attach-render-coalesce");
    let session = format!("attach-render-coalesce-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; i=0; while [ $i -lt 20 ]; do i=$((i + 1)); printf burst-$i-; sleep 0.005; done; printf burst-done; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut stream = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\tRENDER_OUTPUT_META\n");
    let initial = String::from_utf8_lossy(&read_attach_render_frame_body(&mut stream)).to_string();
    assert!(initial.contains("split-ready"), "{initial:?}");
    assert!(!initial.contains("STATUS\t"), "{initial:?}");
    assert!(!initial.contains("\nSNAPSHOT\t"), "{initial:?}");

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "go", "Enter"],
    ));
    let (frames, pushed) = count_attach_render_frames_until_contains(&mut stream, "burst-done");
    assert!(pushed.contains("burst-done"), "{pushed:?}");
    assert!(!pushed.contains("STATUS\t"), "{pushed:?}");
    assert!(!pushed.contains("\nSNAPSHOT\t"), "{pushed:?}");
    assert!(
        frames <= 10,
        "bursty output emitted too many render frames before the final frame: {frames}\n{pushed:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_events_stream_missing_session_errors() {
    let socket = unique_socket("attach-events-missing");
    let session = format!("attach-events-missing-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));

    let missing = format!("{session}-missing");
    let mut stream = attach_events_stream(&socket, &missing);
    assert_eq!(read_socket_line(&mut stream), "ERR missing session\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_events_stream_sends_initial_redraw() {
    let socket = unique_socket("attach-events-initial");
    let session = format!("attach-events-initial-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let mut stream = attach_events_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), "REDRAW\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_events_stream_initial_redraw_goes_only_to_new_subscriber() {
    let socket = unique_socket("attach-events-initial-single");
    let session = format!("attach-events-initial-single-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let mut first = attach_events_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut first), "OK\n");
    assert_eq!(read_socket_line(&mut first), "REDRAW\n");

    let mut second = attach_events_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut second), "OK\n");
    assert_eq!(read_socket_line(&mut second), "REDRAW\n");

    let err = first
        .read(&mut [0_u8; 1])
        .expect_err("first subscriber should not receive second subscriber initial redraw");
    assert!(
        matches!(
            err.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
        "{err:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_events_stream_redraws_after_pane_output() {
    let socket = unique_socket("attach-events-output");
    let session = format!("attach-events-output-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo late:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut stream = attach_events_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), "REDRAW\n");

    assert_success(&dmux(&socket, &["send-keys", "-t", &session, "hello"]));
    assert_eq!(read_socket_line(&mut stream), "REDRAW\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_events_stream_redraws_after_select_pane() {
    let socket = unique_socket("attach-events-select-pane");
    let session = format!("attach-events-select-pane-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut stream = attach_events_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut stream), "OK\n");
    assert_eq!(read_socket_line(&mut stream), "REDRAW\n");

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    assert_eq!(read_socket_line(&mut stream), "REDRAW\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_live_redraws_remaining_pane_after_split_collapses() {
    let socket = unique_socket("attach-live-collapse");
    let session = format!("attach-live-collapse-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; read line; echo remaining:$line; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", "split-ready"]);

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "1"]));
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "visible", "Enter"],
    ));
    let remaining = poll_capture(&socket, &session, "remaining:visible");
    assert!(remaining.contains("remaining:visible"), "{remaining:?}");
    child.wait_for_stdout_contains_all(
        &["remaining:visible"],
        "live attach redraw after split collapse",
    );

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("remaining:visible"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_layout_snapshot_reindexes_after_killing_middle_pane() {
    let socket = unique_socket("attach-layout-kill-middle");
    let session = format!("attach-layout-kill-middle-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf second-ready; sleep 30",
        ],
    ));
    let second = poll_capture(&socket, &session, "second-ready");
    assert!(second.contains("second-ready"), "{second:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-v",
            "--",
            "sh",
            "-c",
            "printf third-ready; sleep 30",
        ],
    ));
    let third = poll_capture(&socket, &session, "third-ready");
    assert!(third.contains("third-ready"), "{third:?}");

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "1"]));

    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", &socket)
        .args(["attach", "-t", &session])
        .stdin(Stdio::null())
        .output()
        .expect("run attach");
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_contains_ordered_line(&stdout, "base-ready", " │ ", "third-ready");
    assert!(!stdout.contains("second-ready"), "{stdout:?}");
    assert!(!stdout.contains("-- pane 0 --"), "{stdout:?}");
    assert!(!stdout.contains("-- pane 1 --"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_keeps_zoomed_split_pane_live() {
    let socket = unique_socket("attach-zoomed-pane-live");
    let session = format!("attach-zoomed-pane-live-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; read line; echo live:$line; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");
    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));

    let mut child = spawn_attached_to_session(&socket, &session, &["split-ready"]);
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"hello\n").expect("write attach input");
        stdin.flush().expect("flush attach input");
    }
    let live = poll_capture(&socket, &session, "live:hello");
    assert!(live.contains("live:hello"), "{live:?}");
    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
    }

    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("-- pane 0 --"), "{stdout:?}");
    assert!(!stdout.contains("-- pane 1 --"), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_prefix_bracket_copies_active_pane_line_when_zoomed() {
    let socket = unique_socket("attach-zoomed-copy-mode");
    let session = format!("attach-zoomed-copy-mode-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-zoom-copy; printf '\\n'; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-zoom-copy");
    assert!(base.contains("base-zoom-copy"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-zoom-copy; printf '\\n'; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-zoom-copy");
    assert!(split.contains("split-zoom-copy"), "{split:?}");
    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));

    let mut child = spawn_attached_to_session(&socket, &session, &["split-zoom-copy"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin
            .write_all(b"\x02[y")
            .expect("write copy-mode entry and copy key");
        stdin.flush().expect("flush copy-mode input");
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut listed = String::new();
    while std::time::Instant::now() < deadline {
        let output = dmux(&socket, &["list-buffers"]);
        assert_success(&output);
        listed = String::from_utf8_lossy(&output.stdout).to_string();
        if listed
            .lines()
            .any(|line| line.ends_with("\t16\tsplit-zoom-copy"))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        listed
            .lines()
            .any(|line| line.ends_with("\t16\tsplit-zoom-copy")),
        "{listed:?}"
    );

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("-- copy mode --"), "{stdout:?}");
    assert!(stdout.contains("-- copy mode: copied to "), "{stdout:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn send_keys_writes_input_to_detached_session() {
    let socket = unique_socket("send-keys");
    let session = format!("send-keys-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "read line; echo got:$line; sleep 30",
        ],
    ));

    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &session, "hello", "Enter"],
    ));

    let captured = poll_capture(&socket, &session, "got:hello");
    assert!(captured.contains("got:hello"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn split_window_creates_second_active_pane() {
    let socket = unique_socket("split-window");
    let session = format!("split-window-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));

    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));

    let panes = dmux(&socket, &["list-panes", "-t", &session]);
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    let active = poll_capture(&socket, &session, "split-ready");
    assert!(active.contains("split-ready"), "{active:?}");
    assert!(!active.contains("base-ready"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn split_window_starts_new_pane_in_invoking_client_cwd() {
    let socket = unique_socket("split-window-client-cwd");
    let session = format!("split-window-client-cwd-{}", std::process::id());
    let dir = unique_temp_file("split-window-client-cwd-dir");
    let file = unique_temp_file("split-window-client-cwd-output");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&file);
    std::fs::create_dir(&dir).expect("create cwd test directory");

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    let mut split = Command::new(env!("CARGO_BIN_EXE_dmux"));
    split.env("DEVMUX_SOCKET", &socket).current_dir(&dir).args([
        "split-window",
        "-t",
        &session,
        "-h",
        "--",
        "sh",
        "-c",
        &format!("pwd > {}; sleep 30", file.display()),
    ]);
    let split = output_with_timeout(split, "run dmux split from cwd", Duration::from_secs(5));
    assert_success(&split);
    let expected_cwd = std::fs::canonicalize(&dir).expect("canonicalize cwd test directory");
    assert!(poll_file_equals(
        &file,
        &format!("{}\n", expected_cwd.display())
    ));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&file);
}

#[test]
fn select_pane_switches_active_capture_target() {
    let socket = unique_socket("select-pane");
    let session = format!("select-pane-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let selected = poll_capture(&socket, &session, "base-ready");
    assert!(selected.contains("base-ready"), "{selected:?}");
    assert!(!selected.contains("split-ready"), "{selected:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn kill_pane_removes_active_pane_and_keeps_session() {
    let socket = unique_socket("kill-pane");
    let session = format!("kill-pane-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session]));

    let panes = dmux(&socket, &["list-panes", "-t", &session]);
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0"]);

    let active = poll_capture(&socket, &session, "base-ready");
    assert!(active.contains("base-ready"), "{active:?}");
    assert!(!active.contains("split-ready"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn kill_pane_by_index_keeps_reindexed_active_pane() {
    let socket = unique_socket("kill-pane-index");
    let session = format!("kill-pane-index-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "0"]));

    let panes = dmux(&socket, &["list-panes", "-t", &session]);
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0"]);

    let active = poll_capture(&socket, &session, "split-ready");
    assert!(active.contains("split-ready"), "{active:?}");
    assert!(!active.contains("base-ready"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn pane_ids_remain_stable_after_index_reassignment() {
    let socket = unique_socket("stable-pane-ids");
    let session = format!("stable-pane-ids-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.id}:#{pane.active}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0:0:0", "1:1:1"]);

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "0"]));

    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.id}:#{pane.active}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0:1:1"]);

    let message = dmux(
        &socket,
        &[
            "display-message",
            "-t",
            &session,
            "-p",
            "#{pane.index}:#{pane.id}",
        ],
    );
    assert_success(&message);
    let message = String::from_utf8_lossy(&message.stdout);
    assert_eq!(message.trim_end(), "0:1");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn kill_pane_terminates_removed_pane_process() {
    let socket = unique_socket("kill-pane-terminates");
    let session = format!("kill-pane-terminates-{}", std::process::id());
    let sentinel = unique_temp_file("kill-pane-terminates");
    let pid_file = unique_temp_file("kill-pane-pid");
    let _ = std::fs::remove_file(&sentinel);
    let _ = std::fs::remove_file(&pid_file);
    let command = format!(
        "printf $$ > {}; trap 'printf terminated > {}; sleep 0.2; exit 0' TERM; printf base-ready; while :; do sleep 0.1; done",
        pid_file.display(),
        sentinel.display()
    );

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", &command],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");
    assert!(
        poll_file_exists(&pid_file),
        "missing {}",
        pid_file.display()
    );
    let pid = std::fs::read_to_string(&pid_file).expect("read pid file");
    let pid = pid.trim();
    assert!(process_exists(pid), "process {pid} should be alive");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "0"]));
    assert!(
        poll_file_exists(&sentinel),
        "missing {}",
        sentinel.display()
    );
    assert!(poll_process_gone(pid), "process {pid} should be gone");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(&sentinel);
    let _ = std::fs::remove_file(&pid_file);
}

#[test]
fn kill_pane_returns_before_slow_process_termination() {
    let socket = unique_socket("kill-pane-fast-return");
    let session = format!("kill-pane-fast-return-{}", std::process::id());
    let pid_file = unique_temp_file("kill-pane-fast-return-pid");
    let _ = std::fs::remove_file(&pid_file);
    let command = format!(
        "trap '' TERM HUP; printf $$ > {}; printf base-ready; while :; do sleep 1; done",
        pid_file.display()
    );

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", &command],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");
    assert!(
        poll_file_exists(&pid_file),
        "missing {}",
        pid_file.display()
    );
    let pid = std::fs::read_to_string(&pid_file).expect("read pid file");
    let pid = pid.trim();
    assert!(process_exists(pid), "process {pid} should be alive");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let started = std::time::Instant::now();
    let output = dmux_with_timeout(
        &socket,
        &["kill-pane", "-t", &session, "-p", "0"],
        Duration::from_secs(1),
    );
    let elapsed = started.elapsed();
    assert_success(&output);
    assert!(
        elapsed < Duration::from_millis(500),
        "kill-pane waited for PTY process termination before updating layout: {elapsed:?}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("unexpected server response"),
        "kill-pane returned an empty control response: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let panes = poll_pane_count(&socket, &session, 1);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0"]);
    assert!(poll_process_gone(pid), "process {pid} should be gone");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(&pid_file);
}

#[test]
fn kill_server_returns_before_slow_process_termination() {
    let socket = unique_socket("kill-server-fast-return");
    let session = format!("kill-server-fast-return-{}", std::process::id());
    let pid_file = unique_temp_file("kill-server-fast-return-pid");
    let _ = std::fs::remove_file(&pid_file);
    let command = format!(
        "trap '' TERM HUP; printf $$ > {}; printf base-ready; while :; do sleep 1; done",
        pid_file.display()
    );

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", &command],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");
    assert!(
        poll_file_exists(&pid_file),
        "missing {}",
        pid_file.display()
    );
    let pid = std::fs::read_to_string(&pid_file).expect("read pid file");
    let pid = pid.trim();
    assert!(process_exists(pid), "process {pid} should be alive");

    let started = std::time::Instant::now();
    let output = dmux_with_timeout(&socket, &["kill-server"], Duration::from_secs(3));
    let elapsed = started.elapsed();
    assert_success(&output);
    assert!(
        elapsed < Duration::from_millis(500),
        "kill-server waited for PTY process termination before returning: {elapsed:?}"
    );
    assert!(poll_process_gone(pid), "process {pid} should be gone");
    let _ = std::fs::remove_file(&pid_file);
}

#[test]
fn kill_pane_terminates_removed_pane_process_group() {
    let socket = unique_socket("kill-pane-process-group");
    let session = format!("kill-pane-process-group-{}", std::process::id());
    let child_pid_file = unique_temp_file("kill-pane-child-pid");
    let _ = std::fs::remove_file(&child_pid_file);
    let command = format!(
        "sh -c 'trap \"\" TERM HUP; printf $$ > {}; while :; do sleep 0.1; done' & printf base-ready; wait",
        child_pid_file.display()
    );

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", &command],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");
    assert!(
        poll_file_exists(&child_pid_file),
        "missing {}",
        child_pid_file.display()
    );
    let child_pid = std::fs::read_to_string(&child_pid_file).expect("read child pid file");
    let child_pid = child_pid.trim();
    assert!(
        process_exists(child_pid),
        "child process {child_pid} should be alive"
    );

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "0"]));
    assert!(
        poll_process_gone(child_pid),
        "child process {child_pid} should be gone"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(&child_pid_file);
}

#[test]
fn new_window_creates_second_active_window() {
    let socket = unique_socket("new-window");
    let session = format!("new-window-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-window; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-window");
    assert!(base.contains("base-window"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf second-window; sleep 30",
        ],
    ));

    let windows = dmux(
        &socket,
        &["list-windows", "-t", &session, "-F", "#{window.index}"],
    );
    assert_success(&windows);
    let windows = String::from_utf8_lossy(&windows.stdout);
    assert_eq!(windows.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    let second = poll_capture(&socket, &session, "second-window");
    assert!(second.contains("second-window"), "{second:?}");
    assert!(!second.contains("base-window"), "{second:?}");

    assert_success(&dmux(
        &socket,
        &["select-window", "-t", &session, "-w", "0"],
    ));
    let selected = poll_capture(&socket, &session, "base-window");
    assert!(selected.contains("base-window"), "{selected:?}");
    assert!(!selected.contains("second-window"), "{selected:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn pane_composition_preserves_pane_ids_and_running_processes() {
    let socket = unique_socket("pane-composition");
    let session = format!("pane-composition-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.id}:#{pane.pid}:#{pane.active}",
        ],
    );
    assert_success(&panes);
    let listed = String::from_utf8_lossy(&panes.stdout);
    let rows = listed
        .lines()
        .map(|line| line.split(':').collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2, "{listed}");
    let base_id = rows[0][1].to_string();
    let base_pid = rows[0][2].to_string();
    let split_id = rows[1][1].to_string();
    let split_pid = rows[1][2].to_string();
    assert_eq!(rows[1][3], "1", "{listed}");

    let source = format!("{session}:.0");
    let destination = format!("{session}:.1");
    assert_success(&dmux(
        &socket,
        &["swap-pane", "-s", &source, "-t", &destination],
    ));
    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.id}:#{pane.pid}:#{pane.active}",
        ],
    );
    assert_success(&panes);
    assert_eq!(
        String::from_utf8_lossy(&panes.stdout)
            .lines()
            .collect::<Vec<_>>(),
        vec![
            format!("0:{split_id}:{split_pid}:1"),
            format!("1:{base_id}:{base_pid}:0"),
        ]
    );

    let base_target = format!("{session}:.%{base_id}");
    assert_success(&dmux(&socket, &["break-pane", "-t", &base_target]));
    let windows = dmux(
        &socket,
        &[
            "list-windows",
            "-t",
            &session,
            "-F",
            "#{window.index}:#{window.panes}",
        ],
    );
    assert_success(&windows);
    assert_eq!(
        String::from_utf8_lossy(&windows.stdout)
            .lines()
            .collect::<Vec<_>>(),
        vec!["0:1", "1:1"]
    );

    let split_target = format!("{session}:0.%{split_id}");
    let join_target = format!("{session}:1.%{base_id}");
    assert_success(&dmux(
        &socket,
        &["join-pane", "-s", &split_target, "-t", &join_target, "-v"],
    ));
    let windows = dmux(
        &socket,
        &[
            "list-windows",
            "-t",
            &session,
            "-F",
            "#{window.index}:#{window.panes}",
        ],
    );
    assert_success(&windows);
    assert_eq!(String::from_utf8_lossy(&windows.stdout).trim(), "0:2");

    let panes = dmux(
        &socket,
        &["list-panes", "-t", &session, "-F", "#{pane.id}:#{pane.pid}"],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert!(panes.contains(&format!("{base_id}:{base_pid}")), "{panes}");
    assert!(
        panes.contains(&format!("{split_id}:{split_pid}")),
        "{panes}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn structured_targets_address_non_active_window() {
    let socket = unique_socket("structured-targets");
    let session = format!("structured-targets-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; while IFS= read -r line; do printf base:$line; done",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf second-ready; while IFS= read -r line; do printf second:$line; done",
        ],
    ));
    let second = poll_capture(&socket, &session, "second-ready");
    assert!(second.contains("second-ready"), "{second:?}");

    let window0 = format!("{session}:0");
    assert_success(&dmux(
        &socket,
        &["send-keys", "-t", &window0, "from-target", "Enter"],
    ));
    let targeted = poll_capture(&socket, &window0, "base:from-target");
    assert!(targeted.contains("base:from-target"), "{targeted:?}");

    let active = poll_capture(&socket, &session, "second-ready");
    assert!(active.contains("second-ready"), "{active:?}");
    assert!(!active.contains("base:from-target"), "{active:?}");

    let failed_select = dmux(&socket, &["select-pane", "-t", &window0, "-p", "99"]);
    assert!(!failed_select.status.success());
    assert!(String::from_utf8_lossy(&failed_select.stderr).contains("missing pane"));
    let active = poll_capture(&socket, &session, "second-ready");
    assert!(active.contains("second-ready"), "{active:?}");
    assert!(!active.contains("base:from-target"), "{active:?}");

    let invalid_zoom = format!("{window0}.99");
    let failed_zoom = dmux(&socket, &["zoom-pane", "-t", &invalid_zoom]);
    assert!(!failed_zoom.status.success());
    assert!(String::from_utf8_lossy(&failed_zoom.stderr).contains("missing pane"));
    let active = poll_capture(&socket, &session, "second-ready");
    assert!(active.contains("second-ready"), "{active:?}");
    assert!(!active.contains("base:from-target"), "{active:?}");

    let windows = dmux(
        &socket,
        &[
            "list-windows",
            "-t",
            &session,
            "-F",
            "#{window.index}:#{window.id}:#{window.active}",
        ],
    );
    assert_success(&windows);
    let listed = String::from_utf8_lossy(&windows.stdout);
    let base_id = listed
        .lines()
        .find_map(|line| line.strip_prefix("0:"))
        .and_then(|rest| rest.split(':').next())
        .expect("base window id")
        .to_string();

    let by_id = format!("{session}:@{base_id}");
    let panes = dmux(
        &socket,
        &["list-panes", "-t", &by_id, "-F", "#{pane.index}"],
    );
    assert_success(&panes);
    assert_eq!(String::from_utf8_lossy(&panes.stdout).trim(), "0");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &by_id,
            "-v",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &by_id, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");
    let panes = dmux(
        &socket,
        &["list-panes", "-t", &by_id, "-F", "#{pane.index}"],
    );
    assert_success(&panes);
    assert_eq!(String::from_utf8_lossy(&panes.stdout).trim(), "0\n1");
    let active = poll_capture(&socket, &session, "second-ready");
    assert!(active.contains("second-ready"), "{active:?}");
    assert!(!active.contains("split-ready"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-window", "-t", &by_id]));
    let windows = dmux(
        &socket,
        &["list-windows", "-t", &session, "-F", "#{window.index}"],
    );
    assert_success(&windows);
    assert_eq!(String::from_utf8_lossy(&windows.stdout).trim(), "0");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn tab_alias_commands_share_window_state() {
    let socket = unique_socket("tab-aliases");
    let session = format!("tab-aliases-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-tab; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-tab");
    assert!(base.contains("base-tab"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-tab",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf second-tab; sleep 30",
        ],
    ));
    let second = poll_capture(&socket, &session, "second-tab");
    assert!(second.contains("second-tab"), "{second:?}");

    let tabs = dmux(
        &socket,
        &["list-tabs", "-t", &session, "-F", "#{tab.index}"],
    );
    assert_success(&tabs);
    let tabs = String::from_utf8_lossy(&tabs.stdout);
    assert_eq!(tabs.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    let status = dmux(
        &socket,
        &[
            "status-line",
            "-t",
            &session,
            "-F",
            "#{tab.index}|#{tab.list}|#{window.index}",
        ],
    );
    assert_success(&status);
    let status = String::from_utf8_lossy(&status.stdout);
    assert_eq!(status.trim_end(), "1|0 [1]|1");

    assert_success(&dmux(&socket, &["select-tab", "-t", &session, "-i", "0"]));
    let selected = poll_capture(&socket, &session, "base-tab");
    assert!(selected.contains("base-tab"), "{selected:?}");
    assert!(!selected.contains("second-tab"), "{selected:?}");

    assert_success(&dmux(&socket, &["kill-tab", "-t", &session, "-i", "1"]));
    let tabs = dmux(
        &socket,
        &["list-tabs", "-t", &session, "-F", "#{tab.index}"],
    );
    assert_success(&tabs);
    let tabs = String::from_utf8_lossy(&tabs.stdout);
    assert_eq!(tabs.lines().collect::<Vec<_>>(), vec!["0"]);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn tab_ids_remain_stable_after_index_reassignment() {
    let socket = unique_socket("stable-tab-ids");
    let session = format!("stable-tab-ids-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-tab-id; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-tab-id");
    assert!(base.contains("base-tab-id"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-tab",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf second-tab-id; sleep 30",
        ],
    ));
    let second = poll_capture(&socket, &session, "second-tab-id");
    assert!(second.contains("second-tab-id"), "{second:?}");

    let status = dmux(
        &socket,
        &[
            "status-line",
            "-t",
            &session,
            "-F",
            "#{window.index}:#{window.id}:#{tab.index}:#{tab.id}",
        ],
    );
    assert_success(&status);
    let status = String::from_utf8_lossy(&status.stdout);
    assert_eq!(status.trim_end(), "1:1:1:1");

    assert_success(&dmux(&socket, &["kill-tab", "-t", &session, "-i", "0"]));

    let status = dmux(
        &socket,
        &[
            "status-line",
            "-t",
            &session,
            "-F",
            "#{window.index}:#{window.id}:#{tab.index}:#{tab.id}",
        ],
    );
    assert_success(&status);
    let status = String::from_utf8_lossy(&status.stdout);
    assert_eq!(status.trim_end(), "0:1:0:1");

    let active = poll_capture(&socket, &session, "second-tab-id");
    assert!(active.contains("second-tab-id"), "{active:?}");
    assert!(!active.contains("base-tab-id"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn window_metadata_rename_select_cycle_and_status_formats_work() {
    let socket = unique_socket("window-metadata");
    let session = format!("window-metadata-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-window; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-window");
    assert!(base.contains("base-window"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf editor-window; sleep 30",
        ],
    ));
    let editor = poll_capture(&socket, &session, "editor-window");
    assert!(editor.contains("editor-window"), "{editor:?}");

    assert_success(&dmux(&socket, &["rename-window", "-t", &session, "editor"]));
    assert_success(&dmux(
        &socket,
        &["select-window", "-t", &session, "-w", "0"],
    ));
    assert_success(&dmux(&socket, &["rename-window", "-t", &session, "base"]));

    let duplicate = dmux(&socket, &["rename-window", "-t", &session, "editor"]);
    assert!(!duplicate.status.success());
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("window name already exists"),
        "{:?}",
        String::from_utf8_lossy(&duplicate.stderr)
    );

    let windows = dmux(
        &socket,
        &[
            "list-windows",
            "-t",
            &session,
            "-F",
            "#{window.index}:#{window.id}:#{window.name}:#{window.active}:#{window.panes}",
        ],
    );
    assert_success(&windows);
    let windows = String::from_utf8_lossy(&windows.stdout);
    assert_eq!(
        windows.lines().collect::<Vec<_>>(),
        vec!["0:0:base:1:1", "1:1:editor:0:1"]
    );
    let default_windows = dmux(&socket, &["list-windows", "-t", &session]);
    assert_success(&default_windows);
    let default_windows = String::from_utf8_lossy(&default_windows.stdout);
    assert_contains_ordered_line(&default_windows, "0", "name=base", "active=1");
    assert_contains_ordered_line(&default_windows, "1", "name=editor", "active=0");
    let mut raw_list = UnixStream::connect(&socket).expect("connect socket");
    raw_list
        .write_all(format!("LIST_WINDOWS\t{session}\n").as_bytes())
        .expect("write raw list-windows request");
    assert_eq!(read_socket_line(&mut raw_list), "OK\n");
    let mut raw_windows = String::new();
    raw_list
        .read_to_string(&mut raw_windows)
        .expect("read raw list-windows body");
    assert_eq!(raw_windows.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    assert_success(&dmux(
        &socket,
        &["select-window", "-t", &session, "-n", "editor"],
    ));
    let selected = poll_capture(&socket, &session, "editor-window");
    assert!(selected.contains("editor-window"), "{selected:?}");

    assert_success(&dmux(
        &socket,
        &["select-window", "-t", &session, "--window-id", "0"],
    ));
    let selected = poll_capture(&socket, &session, "base-window");
    assert!(selected.contains("base-window"), "{selected:?}");

    assert_success(&dmux(&socket, &["next-window", "-t", &session]));
    let selected = poll_capture(&socket, &session, "editor-window");
    assert!(selected.contains("editor-window"), "{selected:?}");
    assert_success(&dmux(&socket, &["next-window", "-t", &session]));
    let selected = poll_capture(&socket, &session, "base-window");
    assert!(selected.contains("base-window"), "{selected:?}");
    assert_success(&dmux(&socket, &["previous-window", "-t", &session]));
    let selected = poll_capture(&socket, &session, "editor-window");
    assert!(selected.contains("editor-window"), "{selected:?}");

    let status = dmux(
        &socket,
        &[
            "status-line",
            "-t",
            &session,
            "-F",
            "#{window.index}:#{window.id}:#{window.name}:#{window.count}:#{tab.name}:#{tab.count}",
        ],
    );
    assert_success(&status);
    assert_eq!(
        String::from_utf8_lossy(&status.stdout).trim_end(),
        "1:1:editor:2:editor:2"
    );

    let empty = dmux(&socket, &["rename-window", "-t", &session, ""]);
    assert!(!empty.status.success());
    assert!(
        String::from_utf8_lossy(&empty.stderr).contains("new name cannot be empty")
            || String::from_utf8_lossy(&empty.stderr).contains("window name cannot be empty"),
        "{:?}",
        String::from_utf8_lossy(&empty.stderr)
    );
    let control = dmux(&socket, &["rename-window", "-t", &session, "bad\nname"]);
    assert!(!control.status.success());
    assert!(
        String::from_utf8_lossy(&control.stderr).contains("control characters"),
        "{:?}",
        String::from_utf8_lossy(&control.stderr)
    );

    assert_success(&dmux(&socket, &["kill-window", "-t", &session]));
    let windows = dmux(
        &socket,
        &[
            "list-windows",
            "-t",
            &session,
            "-F",
            "#{window.index}:#{window.id}:#{window.name}:#{window.active}",
        ],
    );
    assert_success(&windows);
    assert_eq!(
        String::from_utf8_lossy(&windows.stdout)
            .lines()
            .collect::<Vec<_>>(),
        vec!["0:0:base:1"]
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn window_switch_and_kill_sync_visible_window_size() {
    let socket = unique_socket("window-size-sync");
    let session = format!("window-size-sync-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));
    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "100", "-y", "40"],
    ));
    assert_success(&dmux(
        &socket,
        &["new-window", "-t", &session, "--", "sh", "-c", "sleep 30"],
    ));

    assert_success(&dmux(
        &socket,
        &["select-window", "-t", &session, "-w", "0"],
    ));
    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));
    assert_success(&dmux(
        &socket,
        &["select-window", "-t", &session, "-w", "1"],
    ));
    let selected = attach_layout_regions(&socket, &session);
    assert_eq!(selected.len(), 1, "{selected:?}");
    assert_eq!(selected[0].row_end, 24);
    assert_eq!(selected[0].col_end, 83);

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "100", "-y", "40"],
    ));
    assert_success(&dmux(&socket, &["kill-window", "-t", &session]));
    let remaining = attach_layout_regions(&socket, &session);
    assert_eq!(remaining.len(), 1, "{remaining:?}");
    assert_eq!(remaining[0].row_end, 40);
    assert_eq!(remaining[0].col_end, 100);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn window_changes_mark_raw_attach_layout_transitions() {
    let socket = unique_socket("window-raw-epoch");
    let session = format!("window-raw-epoch-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));
    assert_eq!(raw_layout_epoch(&socket, &session), 0);

    assert_success(&dmux(
        &socket,
        &["new-window", "-t", &session, "--", "sh", "-c", "sleep 30"],
    ));
    assert_eq!(raw_layout_epoch(&socket, &session), 1);

    assert_success(&dmux(
        &socket,
        &["select-window", "-t", &session, "-w", "0"],
    ));
    assert_eq!(raw_layout_epoch(&socket, &session), 2);

    assert_success(&dmux(&socket, &["next-window", "-t", &session]));
    assert_eq!(raw_layout_epoch(&socket, &session), 3);

    assert_success(&dmux(&socket, &["previous-window", "-t", &session]));
    assert_eq!(raw_layout_epoch(&socket, &session), 4);

    assert_success(&dmux(&socket, &["kill-window", "-t", &session, "-w", "1"]));
    assert_eq!(raw_layout_epoch(&socket, &session), 4);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn zoomed_select_marks_raw_attach_layout_transitions() {
    let socket = unique_socket("zoom-select-raw-epoch");
    let session = format!("zoom-select-raw-epoch-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));
    assert_eq!(raw_layout_epoch(&socket, &session), 0);

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-v",
            "--",
            "sh",
            "-c",
            "sleep 30",
        ],
    ));
    assert_eq!(raw_layout_epoch(&socket, &session), 1);

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session, "-p", "1"]));
    assert_eq!(raw_layout_epoch(&socket, &session), 2);

    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    assert_eq!(raw_layout_epoch(&socket, &session), 3);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn tab_aliases_support_metadata_rename_select_and_cycle() {
    let socket = unique_socket("tab-metadata");
    let session = format!("tab-metadata-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-tab; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-tab");
    assert!(base.contains("base-tab"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-tab",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf logs-tab; sleep 30",
        ],
    ));
    let logs = poll_capture(&socket, &session, "logs-tab");
    assert!(logs.contains("logs-tab"), "{logs:?}");

    assert_success(&dmux(&socket, &["rename-tab", "-t", &session, "logs"]));
    assert_success(&dmux(&socket, &["select-tab", "-t", &session, "-i", "0"]));
    assert_success(&dmux(&socket, &["rename-tab", "-t", &session, "base"]));

    let tabs = dmux(
        &socket,
        &[
            "list-tabs",
            "-t",
            &session,
            "-F",
            "#{tab.index}:#{tab.id}:#{tab.name}:#{tab.active}:#{tab.panes}",
        ],
    );
    assert_success(&tabs);
    assert_eq!(
        String::from_utf8_lossy(&tabs.stdout)
            .lines()
            .collect::<Vec<_>>(),
        vec!["0:0:base:1:1", "1:1:logs:0:1"]
    );

    assert_success(&dmux(
        &socket,
        &["select-tab", "-t", &session, "-n", "logs"],
    ));
    assert_success(&dmux(&socket, &["previous-tab", "-t", &session]));
    let selected = poll_capture(&socket, &session, "base-tab");
    assert!(selected.contains("base-tab"), "{selected:?}");
    assert_success(&dmux(&socket, &["next-tab", "-t", &session]));
    let selected = poll_capture(&socket, &session, "logs-tab");
    assert!(selected.contains("logs-tab"), "{selected:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn new_window_after_split_uses_full_window_pty_size() {
    let socket = unique_socket("new-window-full-size");
    let session = format!("new-window-full-size-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &["resize-pane", "-t", &session, "-x", "83", "-y", "24"],
    ));

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf new-window-size:; stty size; sleep 30",
        ],
    ));

    let new_window = poll_capture(&socket, &session, "new-window-size:24 83");
    assert!(
        new_window.contains("new-window-size:24 83"),
        "{new_window:?}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn kill_window_removes_active_window_and_keeps_session() {
    let socket = unique_socket("kill-window");
    let session = format!("kill-window-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-window; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-window");
    assert!(base.contains("base-window"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf second-window; sleep 30",
        ],
    ));
    let second = poll_capture(&socket, &session, "second-window");
    assert!(second.contains("second-window"), "{second:?}");

    assert_success(&dmux(&socket, &["kill-window", "-t", &session]));

    let windows = dmux(
        &socket,
        &["list-windows", "-t", &session, "-F", "#{window.index}"],
    );
    assert_success(&windows);
    let windows = String::from_utf8_lossy(&windows.stdout);
    assert_eq!(windows.lines().collect::<Vec<_>>(), vec!["0"]);

    let active = poll_capture(&socket, &session, "base-window");
    assert!(active.contains("base-window"), "{active:?}");
    assert!(!active.contains("second-window"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn kill_window_by_index_keeps_reindexed_active_window() {
    let socket = unique_socket("kill-window-index");
    let session = format!("kill-window-index-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-window; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-window");
    assert!(base.contains("base-window"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf second-window; sleep 30",
        ],
    ));
    let second = poll_capture(&socket, &session, "second-window");
    assert!(second.contains("second-window"), "{second:?}");

    assert_success(&dmux(&socket, &["kill-window", "-t", &session, "-w", "0"]));

    let windows = dmux(
        &socket,
        &["list-windows", "-t", &session, "-F", "#{window.index}"],
    );
    assert_success(&windows);
    let windows = String::from_utf8_lossy(&windows.stdout);
    assert_eq!(windows.lines().collect::<Vec<_>>(), vec!["0"]);

    let active = poll_capture(&socket, &session, "second-window");
    assert!(active.contains("second-window"), "{active:?}");
    assert!(!active.contains("base-window"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn kill_window_terminates_all_removed_window_panes() {
    let socket = unique_socket("kill-window-terminates");
    let session = format!("kill-window-terminates-{}", std::process::id());
    let window_pid_file = unique_temp_file("kill-window-pid");
    let split_pid_file = unique_temp_file("kill-window-split-pid");
    let _ = std::fs::remove_file(&window_pid_file);
    let _ = std::fs::remove_file(&split_pid_file);
    let window_command = format!(
        "printf $$ > {}; printf second-window; while :; do sleep 0.1; done",
        window_pid_file.display()
    );
    let split_command = format!(
        "printf $$ > {}; printf split-window; while :; do sleep 0.1; done",
        split_pid_file.display()
    );

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-window; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-window");
    assert!(base.contains("base-window"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            &window_command,
        ],
    ));
    let second = poll_capture(&socket, &session, "second-window");
    assert!(second.contains("second-window"), "{second:?}");
    assert!(
        poll_file_exists(&window_pid_file),
        "missing {}",
        window_pid_file.display()
    );
    let window_pid = std::fs::read_to_string(&window_pid_file).expect("read window pid file");
    let window_pid = window_pid.trim();
    assert!(
        process_exists(window_pid),
        "window process {window_pid} should be alive"
    );

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            &split_command,
        ],
    ));
    let split = poll_capture(&socket, &session, "split-window");
    assert!(split.contains("split-window"), "{split:?}");
    assert!(
        poll_file_exists(&split_pid_file),
        "missing {}",
        split_pid_file.display()
    );
    let split_pid = std::fs::read_to_string(&split_pid_file).expect("read split pid file");
    let split_pid = split_pid.trim();
    assert!(
        process_exists(split_pid),
        "split process {split_pid} should be alive"
    );

    assert_success(&dmux(&socket, &["kill-window", "-t", &session, "-w", "1"]));
    assert!(
        poll_process_gone(window_pid),
        "window process {window_pid} should be gone"
    );
    assert!(
        poll_process_gone(split_pid),
        "split process {split_pid} should be gone"
    );

    let windows = dmux(
        &socket,
        &["list-windows", "-t", &session, "-F", "#{window.index}"],
    );
    assert_success(&windows);
    let windows = String::from_utf8_lossy(&windows.stdout);
    assert_eq!(windows.lines().collect::<Vec<_>>(), vec!["0"]);

    let active = poll_capture(&socket, &session, "base-window");
    assert!(active.contains("base-window"), "{active:?}");
    assert!(!active.contains("second-window"), "{active:?}");
    assert!(!active.contains("split-window"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(&window_pid_file);
    let _ = std::fs::remove_file(&split_pid_file);
}

#[test]
fn kill_window_rejects_last_window_and_keeps_session_usable() {
    let socket = unique_socket("kill-window-last");
    let session = format!("kill-window-last-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-window; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-window");
    assert!(base.contains("base-window"), "{base:?}");

    let output = dmux(&socket, &["kill-window", "-t", &session]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot kill last window"), "{stderr:?}");

    let windows = dmux(
        &socket,
        &["list-windows", "-t", &session, "-F", "#{window.index}"],
    );
    assert_success(&windows);
    let windows = String::from_utf8_lossy(&windows.stdout);
    assert_eq!(windows.lines().collect::<Vec<_>>(), vec!["0"]);

    let active = poll_capture(&socket, &session, "base-window");
    assert!(active.contains("base-window"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn zoom_pane_marks_active_pane_and_toggles_back() {
    let socket = unique_socket("zoom-pane");
    let session = format!("zoom-pane-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));
    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}:#{window.zoomed_flag}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(
        panes.lines().collect::<Vec<_>>(),
        vec!["0:0:0:1", "1:1:1:1"]
    );

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));
    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}:#{window.zoomed_flag}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(
        panes.lines().collect::<Vec<_>>(),
        vec!["0:0:0:0", "1:1:0:0"]
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn zoom_pane_by_index_selects_requested_pane() {
    let socket = unique_socket("zoom-pane-index");
    let session = format!("zoom-pane-index-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session, "-p", "0"]));
    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}:#{window.zoomed_flag}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(
        panes.lines().collect::<Vec<_>>(),
        vec!["0:1:1:1", "1:0:0:1"]
    );

    let active = poll_capture(&socket, &session, "base-ready");
    assert!(active.contains("base-ready"), "{active:?}");
    assert!(!active.contains("split-ready"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn zoom_pane_follows_selection_and_clears_when_zoomed_pane_is_killed() {
    let socket = unique_socket("zoom-pane-select-kill");
    let session = format!("zoom-pane-select-kill-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));
    assert_success(&dmux(&socket, &["select-pane", "-t", &session, "-p", "0"]));
    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}:#{window.zoomed_flag}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(
        panes.lines().collect::<Vec<_>>(),
        vec!["0:1:1:1", "1:0:0:1"]
    );

    assert_success(&dmux(&socket, &["kill-pane", "-t", &session, "-p", "0"]));
    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}:#{window.zoomed_flag}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0:1:0:0"]);

    let active = poll_capture(&socket, &session, "split-ready");
    assert!(active.contains("split-ready"), "{active:?}");
    assert!(!active.contains("base-ready"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn zoom_pane_follows_new_split_when_already_zoomed() {
    let socket = unique_socket("zoom-pane-split");
    let session = format!("zoom-pane-split-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}:#{window.zoomed_flag}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(
        panes.lines().collect::<Vec<_>>(),
        vec!["0:0:0:1", "1:1:1:1"]
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn status_line_reports_active_window_pane_and_zoom_state() {
    let socket = unique_socket("status-line");
    let session = format!("status-line-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");

    assert_success(&dmux(
        &socket,
        &[
            "new-window",
            "-t",
            &session,
            "--",
            "sh",
            "-c",
            "printf window-ready; sleep 30",
        ],
    ));
    let window = poll_capture(&socket, &session, "window-ready");
    assert!(window.contains("window-ready"), "{window:?}");

    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");
    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));

    let output = dmux(
        &socket,
        &[
            "status-line",
            "-t",
            &session,
            "-F",
            "#{session.name}|#{window.index}|#{window.list}|#{pane.index}|#{pane.zoomed}|#{window.zoomed_flag}",
        ],
    );
    assert_success(&output);
    let line = String::from_utf8_lossy(&output.stdout);
    assert_eq!(line.trim_end(), format!("{session}|1|0 [1]|1|1|1"));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn display_message_prints_status_format() {
    let socket = unique_socket("display-message");
    let session = format!("display-message-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let output = dmux(
        &socket,
        &[
            "display-message",
            "-t",
            &session,
            "-p",
            "#{session.name}:#{window.index}:#{pane.index}",
        ],
    );
    assert_success(&output);
    let line = String::from_utf8_lossy(&output.stdout);
    assert_eq!(line.trim_end(), format!("{session}:0:0"));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn display_message_preserves_token_like_session_names() {
    let socket = unique_socket("display-message-token-name");
    let session = format!("#{{pane.index}}-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let output = dmux(
        &socket,
        &["display-message", "-t", &session, "-p", "#{session.name}"],
    );
    assert_success(&output);
    let line = String::from_utf8_lossy(&output.stdout);
    assert_eq!(line.trim_end(), session);

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn status_line_and_display_message_expose_buffer_fields_safely() {
    let socket = unique_socket("status-message-buffer");
    let session = format!("status-message-buffer-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");
    save_buffer_text(&socket, &session, "#{buffer.latest}", "#{pane.index}\n");

    let output = dmux(
        &socket,
        &[
            "status-line",
            "-t",
            &session,
            "-F",
            "#{buffer.count}:#{buffer.name}:#{buffer.bytes}:#{buffer.lines}:#{buffer.latest}:#{buffer.preview}:#{missing.token}",
        ],
    );
    assert_success(&output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim_end(),
        "1:#{buffer.latest}:14:1:1:#{pane.index}:#{missing.token}"
    );

    let message = dmux(
        &socket,
        &["display-message", "-t", &session, "-p", "#{buffer.preview}"],
    );
    assert_success(&message);
    assert_eq!(
        String::from_utf8_lossy(&message.stdout).trim_end(),
        "#{pane.index}"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn status_line_uses_default_format() {
    let socket = unique_socket("status-line-default");
    let session = format!("status-line-default-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let output = dmux(&socket, &["status-line", "-t", &session]);
    assert_success(&output);
    let line = String::from_utf8_lossy(&output.stdout);
    assert_eq!(line.trim_end(), format!("{session} [0] pane 0"));
    assert!(!line.contains("C-b ? help"), "{line:?}");

    let output = dmux(
        &socket,
        &["status-line", "-t", &session, "-F", "#{status.help}"],
    );
    assert_success(&output);
    let line = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        line.trim_end(),
        "prefix C-b | C-b ? help | : command | [ copy"
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn status_line_and_display_message_report_missing_session() {
    let socket = unique_socket("status-line-missing");

    let output = dmux(&socket, &["status-line", "-t", "missing"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing session"), "{stderr:?}");

    let output = dmux(
        &socket,
        &["display-message", "-t", "missing", "-p", "#{session.name}"],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing session"), "{stderr:?}");

    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn session_lifecycle_lists_renames_and_preserves_state() {
    let socket = unique_socket("session-lifecycle");
    let session = format!("session-lifecycle-{}", std::process::id());
    let renamed = format!("session-lifecycle-renamed-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let legacy = dmux(&socket, &["ls"]);
    assert_success(&legacy);
    assert_eq!(
        String::from_utf8_lossy(&legacy.stdout),
        format!("{session}\n")
    );

    let listed = dmux(
        &socket,
        &[
            "list-sessions",
            "-F",
            "#{session.name} #{session.windows} #{session.attached} #{client.count}",
        ],
    );
    assert_success(&listed);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        format!("{session} 1 0 0\n")
    );

    assert_success(&dmux(
        &socket,
        &["rename-session", "-t", &session, &renamed],
    ));

    let old_capture = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert!(!old_capture.status.success());
    assert!(String::from_utf8_lossy(&old_capture.stderr).contains("missing session"));

    let new_capture = dmux(&socket, &["capture-pane", "-t", &renamed, "-p"]);
    assert_success(&new_capture);
    assert!(String::from_utf8_lossy(&new_capture.stdout).contains("ready"));

    let display = dmux(
        &socket,
        &[
            "display-message",
            "-t",
            &renamed,
            "-p",
            "#{session.name} #{session.attached_count}",
        ],
    );
    assert_success(&display);
    assert_eq!(
        String::from_utf8_lossy(&display.stdout),
        format!("{renamed} 0\n")
    );

    assert_success(&dmux(&socket, &["kill-session", "-t", &renamed]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn concurrent_new_allows_only_one_session_with_a_name() {
    let socket = unique_socket("concurrent-new");
    let bootstrap = format!("concurrent-new-bootstrap-{}", std::process::id());
    let target = format!("concurrent-new-target-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &bootstrap, "--", "sh", "-c", "sleep 30"],
    ));

    let handles = (0..8)
        .map(|_| {
            let socket = socket.clone();
            let target = target.clone();
            std::thread::spawn(move || {
                dmux(
                    &socket,
                    &["new", "-d", "-s", &target, "--", "sh", "-c", "sleep 30"],
                )
            })
        })
        .collect::<Vec<_>>();
    let outputs = handles
        .into_iter()
        .map(|handle| handle.join().expect("join concurrent new"))
        .collect::<Vec<_>>();
    let successes = outputs
        .iter()
        .filter(|output| output.status.success())
        .count();
    assert_eq!(successes, 1, "outputs: {outputs:?}");
    for output in outputs.iter().filter(|output| !output.status.success()) {
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("already exists"),
            "stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let listed = dmux(&socket, &["list-sessions", "-F", "#{session.name}"]);
    assert_success(&listed);
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert_eq!(listed.lines().filter(|line| *line == target).count(), 1);

    assert_success(&dmux(&socket, &["kill-session", "-t", &target]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &bootstrap]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn list_clients_and_detach_client_close_attach_without_killing_session() {
    let socket = unique_socket("client-lifecycle");
    let session = format!("client-lifecycle-{}", std::process::id());
    let renamed = format!("client-lifecycle-renamed-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let child = spawn_attached_to_session(&socket, &session, &["ready"]);

    let clients = dmux(
        &socket,
        &[
            "list-clients",
            "-t",
            &session,
            "-F",
            "#{client.id} #{client.session} #{client.type} #{client.attached}",
        ],
    );
    assert_success(&clients);
    let clients_text = String::from_utf8_lossy(&clients.stdout);
    assert_eq!(clients_text.lines().count(), 1, "{clients_text:?}");
    let line = clients_text.lines().next().expect("client line");
    let parts = line.split_whitespace().collect::<Vec<_>>();
    assert_eq!(parts[1], session);
    assert_eq!(parts[2], "raw");
    assert_eq!(parts[3], "1");

    assert_success(&dmux(
        &socket,
        &["rename-session", "-t", &session, &renamed],
    ));
    assert_success(&dmux(&socket, &["detach-client", "-c", parts[0]]));
    let output = assert_child_exits_within(child, "attach after detach-client");
    assert_success(&output);

    let old_capture = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert!(!old_capture.status.success());
    assert!(String::from_utf8_lossy(&old_capture.stderr).contains("missing session"));

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));

    let capture = dmux(&socket, &["capture-pane", "-t", &renamed, "-p"]);
    assert_success(&capture);
    assert!(String::from_utf8_lossy(&capture.stdout).contains("ready"));

    let clients = dmux(&socket, &["list-clients", "-t", &renamed]);
    assert_success(&clients);
    assert_eq!(String::from_utf8_lossy(&clients.stdout), "");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &renamed]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn list_clients_reports_and_detaches_event_and_render_streams() {
    let socket = unique_socket("client-stream-types");
    let session = format!("client-stream-types-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));

    let mut events = attach_events_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut events), "OK\n");
    assert_eq!(read_socket_line(&mut events), "REDRAW\n");
    let mut render = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut render), "OK\tRENDER_OUTPUT_META\n");
    let _initial_render = read_attach_render_frame_body(&mut render);

    let clients = dmux(
        &socket,
        &[
            "list-clients",
            "-t",
            &session,
            "-F",
            "#{client.id} #{client.type} #{client.width}x#{client.height} #{client.attached}",
        ],
    );
    assert_success(&clients);
    let clients_text = String::from_utf8_lossy(&clients.stdout);
    assert!(
        clients_text.lines().any(|line| line.contains(" event ")),
        "{clients_text}"
    );
    assert!(
        clients_text.lines().any(|line| line.contains(" render ")),
        "{clients_text}"
    );
    assert!(
        clients_text.lines().all(|line| line.ends_with(" 1")),
        "{clients_text}"
    );

    let event_id = clients_text
        .lines()
        .find(|line| line.contains(" event "))
        .and_then(|line| line.split_whitespace().next())
        .expect("event client id")
        .to_string();
    assert_success(&dmux(&socket, &["detach-client", "-c", &event_id]));

    let clients = dmux(
        &socket,
        &["list-clients", "-t", &session, "-F", "#{client.type}"],
    );
    assert_success(&clients);
    let clients_text = String::from_utf8_lossy(&clients.stdout);
    assert!(
        !clients_text.lines().any(|line| line == "event"),
        "{clients_text}"
    );
    assert!(
        clients_text.lines().any(|line| line == "render"),
        "{clients_text}"
    );

    assert_success(&dmux(&socket, &["detach-client", "-t", &session]));
    let clients = dmux(&socket, &["list-clients", "-t", &session]);
    assert_success(&clients);
    assert_eq!(String::from_utf8_lossy(&clients.stdout), "");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn rename_alias_expires_when_layout_reconnect_is_abandoned() {
    let socket = unique_socket("rename-alias-abandoned-reconnect");
    let session = format!("rename-alias-abandoned-reconnect-{}", std::process::id());
    let renamed = format!(
        "rename-alias-abandoned-reconnect-new-{}",
        std::process::id()
    );

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let mut raw_attach = UnixStream::connect(&socket).expect("connect raw attach stream");
    raw_attach
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set raw attach read timeout");
    raw_attach
        .write_all(format!("ATTACH\t{session}\n").as_bytes())
        .expect("write raw attach request");
    let response = read_socket_line(&mut raw_attach);
    assert!(response.starts_with("OK\tLIVE\t"), "{response:?}");

    assert_success(&dmux(
        &socket,
        &["rename-session", "-t", &session, &renamed],
    ));
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &renamed,
            "-h",
            "--",
            "sh",
            "-c",
            "sleep 30",
        ],
    ));
    let _ = try_read_socket_line(&mut raw_attach);
    drop(raw_attach);

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut last_stderr = String::new();
    loop {
        let output = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
        if !output.status.success() {
            last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if last_stderr.contains("missing session") {
                break;
            }
        }
        if std::time::Instant::now() >= deadline {
            panic!("old session alias did not expire; last stderr:\n{last_stderr}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &renamed]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attached_client_commands_survive_session_rename() {
    let socket = unique_socket("attached-rename");
    let session = format!("attached-rename-{}", std::process::id());
    let renamed = format!("attached-rename-new-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["ready"]);
    assert_success(&dmux(
        &socket,
        &["rename-session", "-t", &session, &renamed],
    ));

    let listed = dmux(&socket, &["ls"]);
    assert_success(&listed);
    assert_eq!(
        String::from_utf8_lossy(&listed.stdout),
        format!("{renamed}\n")
    );

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02%").expect("write split prefix");
        stdin.flush().expect("flush split prefix");
    }
    let panes = poll_pane_count(&socket, &renamed, 2);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0", "1"]);

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    let old_capture = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert!(!old_capture.status.success());
    assert!(String::from_utf8_lossy(&old_capture.stderr).contains("missing session"));

    assert_success(&dmux(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"],
    ));

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &renamed]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn rename_session_accepts_attached_rename_alias_again() {
    let socket = unique_socket("attached-rename-alias-again");
    let session = format!("attached-rename-alias-again-{}", std::process::id());
    let renamed = format!("attached-rename-alias-again-new-{}", std::process::id());
    let renamed_again = format!("attached-rename-alias-again-newer-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");

    let mut child = spawn_attached_to_session(&socket, &session, &["ready"]);
    assert_success(&dmux(
        &socket,
        &["rename-session", "-t", &session, &renamed],
    ));
    let old_alias_capture = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert_success(&old_alias_capture);
    assert!(String::from_utf8_lossy(&old_alias_capture.stdout).contains("ready"));

    assert_success(&dmux(
        &socket,
        &["rename-session", "-t", &session, &renamed_again],
    ));

    let old_alias_capture = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert_success(&old_alias_capture);
    assert!(String::from_utf8_lossy(&old_alias_capture.stdout).contains("ready"));
    let previous_name_alias_capture = dmux(&socket, &["capture-pane", "-t", &renamed, "-p"]);
    assert_success(&previous_name_alias_capture);
    assert!(String::from_utf8_lossy(&previous_name_alias_capture.stdout).contains("ready"));

    {
        let stdin = child.stdin_mut("attach stdin");
        stdin.write_all(b"\x02d").expect("write detach input");
        stdin.flush().expect("flush detach input");
    }
    let output = wait_for_child_exit(child);
    assert_success(&output);

    assert_success(&dmux(&socket, &["kill-session", "-t", &renamed_again]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn status_line_reports_attached_count_for_current_session() {
    let socket = unique_socket("status-attached-count");
    let session = format!("status-attached-count-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf ready; sleep 30",
        ],
    ));
    let ready = poll_capture(&socket, &session, "ready");
    assert!(ready.contains("ready"), "{ready:?}");
    let child = spawn_attached_to_session(&socket, &session, &["ready"]);

    let status = dmux(
        &socket,
        &[
            "status-line",
            "-t",
            &session,
            "-F",
            "#{session.attached} #{session.attached_count} #{client.count}",
        ],
    );
    assert_success(&status);
    assert_eq!(String::from_utf8_lossy(&status.stdout), "1 1 1\n");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    let output = assert_child_exits_within(child, "attach after kill-session");
    assert_success(&output);
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn attach_render_status_updates_for_client_count_and_messages() {
    let socket = unique_socket("attach-render-status-message");
    let session = format!("attach-render-status-message-{}", std::process::id());

    assert_success(&dmux(
        &socket,
        &[
            "new",
            "-d",
            "-s",
            &session,
            "--",
            "sh",
            "-c",
            "printf base-ready; sleep 30",
        ],
    ));
    let base = poll_capture(&socket, &session, "base-ready");
    assert!(base.contains("base-ready"), "{base:?}");
    assert_success(&dmux(
        &socket,
        &[
            "split-window",
            "-t",
            &session,
            "-h",
            "--",
            "sh",
            "-c",
            "printf split-ready; sleep 30",
        ],
    ));
    let split = poll_capture(&socket, &session, "split-ready");
    assert!(split.contains("split-ready"), "{split:?}");

    let mut render = attach_render_stream(&socket, &session);
    assert_eq!(read_socket_line(&mut render), "OK\tRENDER_OUTPUT_META\n");
    let initial = read_attach_render_frame_body_until_contains(&mut render, "clients 0");
    assert!(initial.contains("base-ready") || initial.contains("split-ready"));

    let mut child = spawn_attached_to_session(&socket, &session, &["clients 1"]);
    let _updated = read_attach_render_frame_body_until_contains(&mut render, "clients 1");
    save_buffer_text(&socket, &session, "render-buffer", "#{pane.index}\n");
    let _buffer_update = read_attach_render_frame_body_until_contains(&mut render, "buffers 1");

    let message = dmux(
        &socket,
        &[
            "display-message",
            "-t",
            &session,
            "-p",
            "buffer=#{buffer.name}|#{missing.token}",
        ],
    );
    assert_success(&message);
    let frame = read_attach_render_frame_body_until_contains(&mut render, "buffer=");
    assert!(
        frame.contains("buffer=render-buffer|#{missing.token}"),
        "{frame:?}"
    );

    child
        .stdin_mut("detach attached status child")
        .write_all(b"\x02d")
        .expect("write detach input");
    let output = wait_for_child_exit(child);
    assert_success(&output);
    let _detached = read_attach_render_frame_body_until_contains(&mut render, "clients 0");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn rename_session_rejects_invalid_and_duplicate_names() {
    let socket = unique_socket("rename-session-validation");
    let first = format!("rename-session-first-{}", std::process::id());
    let second = format!("rename-session-second-{}", std::process::id());

    assert_success(&dmux(&socket, &["new", "-d", "-s", &first]));
    assert_success(&dmux(&socket, &["new", "-d", "-s", &second]));

    let duplicate = dmux(&socket, &["rename-session", "-t", &first, &second]);
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already exists"));

    let empty = dmux(&socket, &["rename-session", "-t", &first, ""]);
    assert!(!empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr).contains("cannot be empty"));

    let control = dmux(&socket, &["rename-session", "-t", &first, "bad\u{1}name"]);
    assert!(!control.status.success());
    assert!(String::from_utf8_lossy(&control.stderr).contains("control characters"));

    let colon = dmux(&socket, &["rename-session", "-t", &first, "bad:target"]);
    assert!(!colon.status.success());
    assert!(String::from_utf8_lossy(&colon.stderr).contains("cannot contain ':'"));

    let bad_new = dmux(&socket, &["new", "-d", "-s", "bad\u{1}name"]);
    assert!(!bad_new.status.success());
    assert!(String::from_utf8_lossy(&bad_new.stderr).contains("control characters"));

    let bad_new_colon = dmux(&socket, &["new", "-d", "-s", "bad:target"]);
    assert!(!bad_new_colon.status.success());
    assert!(String::from_utf8_lossy(&bad_new_colon.stderr).contains("cannot contain ':'"));

    assert_success(&dmux(&socket, &["kill-session", "-t", &first]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &second]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
