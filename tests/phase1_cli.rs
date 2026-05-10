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
