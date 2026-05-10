# Statusline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a statusline formatting surface that reports session, active window, active pane, and zoom state without writing UI bytes into pane PTYs.

**Architecture:** Keep this slice server-side and command-driven because the current attach path streams child PTY output directly. Add `status-line` for rendering the default or caller-provided status format, and `display-message -p` for format expansion. Reuse the active window/pane metadata already maintained by the server; do not add config files or a full attach overlay in this slice.

**Tech Stack:** Rust standard library, existing CLI parser, existing Unix socket protocol, existing server `Session`/`WindowSet` state, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Surface

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI tests**

Add tests in `src/cli.rs`:

```rust
#[test]
fn parses_status_line_target() {
    let command = parse_args(["dmux", "status-line", "-t", "dev"]).unwrap();
    assert_eq!(
        command,
        Command::StatusLine {
            session: "dev".to_string(),
            format: None,
        }
    );
}

#[test]
fn parses_status_line_format() {
    let command = parse_args([
        "dmux",
        "status-line",
        "-t",
        "dev",
        "-F",
        "#{session.name}:#{window.index}:#{pane.index}",
    ])
    .unwrap();
    assert_eq!(
        command,
        Command::StatusLine {
            session: "dev".to_string(),
            format: Some("#{session.name}:#{window.index}:#{pane.index}".to_string()),
        }
    );
}

#[test]
fn parses_display_message_print_format() {
    let command = parse_args([
        "dmux",
        "display-message",
        "-t",
        "dev",
        "-p",
        "#{session.name}:#{pane.index}",
    ])
    .unwrap();
    assert_eq!(
        command,
        Command::DisplayMessage {
            session: "dev".to_string(),
            format: "#{session.name}:#{pane.index}".to_string(),
        }
    );
}
```

- [x] **Step 2: Write failing protocol tests**

Add tests in `src/protocol.rs`:

```rust
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
```

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test --bin dmux status_line
cargo test --bin dmux display_message
```

Expected: FAIL because statusline commands and protocol requests do not exist.

- [x] **Step 4: Implement CLI and protocol**

Add:

```rust
Command::StatusLine { session: String, format: Option<String> }
Command::DisplayMessage { session: String, format: String }
Request::StatusLine { session: String, format: Option<String> }
Request::DisplayMessage { session: String, format: String }
```

Parse `status-line -t <session> [-F <format>]` and `display-message -t <session> -p <format>`. Encode default status requests as `STATUS_LINE\t<session>\n`, formatted status requests as `STATUS_LINE_FORMAT\t<session>\t<hex-utf8-format>\n`, and display-message requests as `DISPLAY_MESSAGE\t<session>\t<hex-utf8-format>\n`.

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --bin dmux status_line
cargo test --bin dmux display_message
```

Expected: PASS.

### Task 2: Server Status Formatting

**Files:**
- Modify: `src/main.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing integration tests**

Add tests in `tests/phase1_cli.rs`:

```rust
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
    assert!(poll_capture(&socket, &session, "base-ready").contains("base-ready"));

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
    assert!(poll_capture(&socket, &session, "window-ready").contains("window-ready"));

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
    assert!(poll_capture(&socket, &session, "ready").contains("ready"));

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
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --test phase1_cli status_line
cargo test --test phase1_cli display_message
```

Expected: FAIL because the server does not handle statusline requests.

- [x] **Step 3: Implement status context and formatter**

Add a `StatusContext` populated from the active session/window/pane:

```rust
struct StatusContext {
    session_name: String,
    window_index: usize,
    window_count: usize,
    pane_index: usize,
    pane_zoomed: bool,
    window_zoomed: bool,
}
```

Add a formatter that replaces:

```text
#{session.name}
#{window.index}
#{window.list}
#{pane.index}
#{pane.zoomed}
#{window.zoomed_flag}
```

Render `#{window.list}` as space-separated window indexes, with the active window wrapped in brackets, for example `0 [1]`.

Default `status-line` format:

```text
#{session.name} #{window.list} pane #{pane.index}
```

- [x] **Step 4: Verify GREEN**

Run:

```bash
cargo test --test phase1_cli status_line
cargo test --test phase1_cli display_message
```

Expected: PASS.

### Task 3: Docs, Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-statusline.md`

- [x] **Step 1: Document the commands**

Add `dmux status-line -t <name> [-F <format>]` and `dmux display-message -t <name> -p <format>` to README. Mention server-side statusline formatting groundwork and note that attach-time statusline rendering is not implemented yet.

- [x] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
cargo test
forbidden-keyword scan
git diff --check
```

Expected: formatting and tests pass; the forbidden-keyword scan prints no matches; whitespace check passes.

- [x] **Step 3: Request subagent review**

Dispatch a read-only review subagent over `git diff origin/main`. Apply technically valid blocking or important findings and rerun full verification.

- [ ] **Step 4: Commit and open draft PR**

Run:

```bash
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-statusline.md
git commit -m "feat: add statusline formatting"
git push -u origin devmux-statusline
gh pr create --draft --base main --head devmux-statusline --title "Add statusline formatting" --body "<summary and validation>"
```
