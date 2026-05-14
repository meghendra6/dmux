use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

fn dmux(socket: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dmux"))
        .env("DEVMUX_SOCKET", socket)
        .args(args)
        .output()
        .expect("run dmux")
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
            .any(|line| !line.is_empty() && line.chars().all(|ch| ch == '-')),
        "{text:?}"
    );
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

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
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
    assert!(stdout.contains("split-window"), "{stdout:?}");
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
    assert!(stdout.contains("C-b ? help"), "{stdout:?}");

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
    assert_contains_ordered_line(&stdout, "base-ready", " | ", "split-ready");
    assert!(!stdout.contains("-- pane 0 --"), "{stdout:?}");
    assert!(!stdout.contains("-- pane 1 --"), "{stdout:?}");

    let captured = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert_success(&captured);
    let captured = String::from_utf8_lossy(&captured.stdout);
    assert!(!captured.contains(" | "), "{captured:?}");

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
    assert!(stdout.contains(" | "), "{stdout:?}");
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
fn active_attach_exits_when_pane_process_exits() {
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

    let child = spawn_attached_to_session(&socket, &session, &["base-ready"]);

    let output = assert_child_exits_within(child, "raw attach after pane process exit");
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
    assert!(stdout.contains(" | "), "{stdout:?}");

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
            .any(|line| !line.is_empty() && line.chars().all(|ch| ch == '-')),
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

    let mut child = spawn_attached_to_session(&socket, &session, &["base-ready", " | "]);

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
    assert!(stdout.contains(" | "), "{stdout:?}");

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

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut listed = String::new();
    while std::time::Instant::now() < deadline {
        let output = dmux(&socket, &["list-buffers"]);
        assert_success(&output);
        listed = String::from_utf8_lossy(&output.stdout).to_string();
        if listed
            .lines()
            .any(saved_buffer_preview_contains_composed_copy)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        listed
            .lines()
            .any(saved_buffer_preview_contains_composed_copy),
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

fn saved_buffer_preview_contains_composed_copy(line: &str) -> bool {
    line.ends_with("\t23\tbase-copy | split-copy")
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
    assert!(body.contains("REGION\t0\t0\t1\t0\t10\n"), "{body:?}");
    assert!(body.contains("REGION\t1\t0\t1\t13\t24\n"), "{body:?}");
    assert!(body.contains("SNAPSHOT\t"), "{body:?}");
    assert!(body.contains("base-ready | split-ready\r\n"), "{body:?}");

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
    assert!(
        plain_body.contains("base-ready | split-ready\r\n"),
        "{plain_body:?}"
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
    assert_contains_ordered_line(&stdout, "base-ready", " | ", "third-ready");
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
    assert!(!process_exists(pid), "process {pid} should be gone");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
    let _ = std::fs::remove_file(&sentinel);
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

    let windows = dmux(&socket, &["list-windows", "-t", &session]);
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

    let windows = dmux(&socket, &["list-windows", "-t", &session]);
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

    let windows = dmux(&socket, &["list-windows", "-t", &session]);
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

    let windows = dmux(&socket, &["list-windows", "-t", &session]);
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

    let windows = dmux(&socket, &["list-windows", "-t", &session]);
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
    assert_eq!(line.trim_end(), "C-b ? help");

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
