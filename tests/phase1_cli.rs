use std::process::{Command, Output, Stdio};
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
fn attach_reports_missing_session() {
    let socket = unique_socket("missing-attach");
    let output = dmux(&socket, &["attach", "-t", "missing"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing session"), "{stderr:?}");

    let _ = dmux(&socket, &["kill-server"]);
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
