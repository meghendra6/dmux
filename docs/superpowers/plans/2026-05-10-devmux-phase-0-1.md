# DevMux Phase 0/1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first usable `dmux` binary: a Rust CLI plus background server that can create a detached single PTY session, keep it alive without clients, capture output, attach interactively, and kill sessions/server.

**Architecture:** Use a single binary with a hidden `__server` subcommand. Public CLI commands connect to a Unix domain socket; if no server is running, they spawn `dmux __server` in the background. The server owns PTY file descriptors and session state, while clients only forward terminal input/output.

**Tech Stack:** Rust standard library only for v1 bootstrap; POSIX FFI for `forkpty`, `kill`, and basic terminal detection; Unix domain sockets for local IPC.

---

### Task 1: Cargo Workspace And CLI Contract Tests

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `tests/phase1_cli.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `Cargo.toml` with package metadata and no dependencies:

```toml
[package]
name = "devmux"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "dmux"
path = "src/main.rs"
```

Create `tests/phase1_cli.rs`:

```rust
use std::process::{Command, Output};
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
    std::env::temp_dir().join(format!("dmux-{name}-{}-{nanos}.sock", std::process::id()))
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

    std::thread::sleep(std::time::Duration::from_millis(1500));

    let output = dmux(&socket, &["capture-pane", "-t", &session, "-p"]);
    assert_success(&output);
    let captured = String::from_utf8_lossy(&output.stdout);
    assert!(captured.contains("ready"), "{captured:?}");
    assert!(captured.contains("done"), "{captured:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}

#[test]
fn list_sessions_reports_created_session() {
    let socket = unique_socket("list");
    let session = format!("list-{}", std::process::id());

    let output = dmux(&socket, &["new", "-d", "-s", &session, "--", "sh", "-c", "sleep 30"]);
    assert_success(&output);

    let output = dmux(&socket, &["ls"]);
    assert_success(&output);
    let sessions = String::from_utf8_lossy(&output.stdout);
    assert!(sessions.lines().any(|line| line == session), "{sessions:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

Create `src/main.rs` with a deliberately incomplete implementation:

```rust
fn main() {
    eprintln!("dmux is not implemented yet");
    std::process::exit(2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test phase1_cli`

Expected: both tests fail because `dmux` exits with status 2 and prints `dmux is not implemented yet`.

- [ ] **Step 3: Commit the failing test contract**

Run:

```bash
git add Cargo.toml src/main.rs tests/phase1_cli.rs
git commit -m "test: define phase 1 cli contract"
```

---

### Task 2: CLI Parser, Socket Paths, And Server Bootstrap

**Files:**
- Modify: `src/main.rs`
- Create: `src/cli.rs`
- Create: `src/paths.rs`
- Create: `src/protocol.rs`

- [ ] **Step 1: Add parser unit tests**

Create parser tests in `src/cli.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_detached_new_session_with_command() {
        let command = parse_args([
            "dmux", "new", "-d", "-s", "dev", "--", "sh", "-c", "echo ok",
        ])
        .unwrap();

        assert_eq!(
            command,
            Command::New {
                session: "dev".to_string(),
                detach: true,
                command: vec!["sh".to_string(), "-c".to_string(), "echo ok".to_string()],
            }
        );
    }

    #[test]
    fn parses_capture_pane_print() {
        let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p"]).unwrap();
        assert_eq!(command, Command::CapturePane { session: "dev".to_string() });
    }

    #[test]
    fn rejects_missing_session_target() {
        let err = parse_args(["dmux", "kill-session"]).unwrap_err();
        assert!(err.contains("-t"));
    }
}
```

- [ ] **Step 2: Run parser tests to verify they fail**

Run: `cargo test cli::tests`

Expected: compilation fails because `src/cli.rs` and `parse_args` do not exist.

- [ ] **Step 3: Implement minimal parser and protocol helpers**

Implement:

```rust
pub enum Command {
    New { session: String, detach: bool, command: Vec<String> },
    Attach { session: String },
    ListSessions,
    CapturePane { session: String },
    KillSession { session: String },
    KillServer,
    Server,
}
```

Support public commands:

```text
new | new-session
attach | attach-session
ls | list-sessions
capture-pane
kill-session
kill-server
__server
```

Implement protocol line helpers using tab fields and ASCII unit separator for command arguments:

```text
NEW\t<session>\t<argc>\t<arg1>\x1f<arg2>...\n
ATTACH\t<session>\n
LIST\n
CAPTURE\t<session>\n
KILL\t<session>\n
KILL_SERVER\n
```

Implement socket path selection in `src/paths.rs`:

1. Use `DEVMUX_SOCKET` when set.
2. Otherwise use `$TMPDIR/devmux-<uid>/default.sock`.
3. Fall back to `std::env::temp_dir()/devmux/default.sock`.

- [ ] **Step 4: Run parser tests to verify they pass**

Run: `cargo test cli::tests`

Expected: parser tests pass.

- [ ] **Step 5: Commit parser/bootstrap support**

Run:

```bash
git add src/main.rs src/cli.rs src/paths.rs src/protocol.rs
git commit -m "feat: add dmux cli parser and protocol"
```

---

### Task 3: Single PTY Session Server

**Files:**
- Modify: `src/main.rs`
- Create: `src/pty.rs`
- Create: `src/server.rs`

- [ ] **Step 1: Add PTY/server unit tests where possible**

Add focused tests for protocol parsing and session command defaults:

```rust
#[test]
fn new_session_defaults_to_user_shell_when_command_is_empty() {
    let spec = SpawnSpec::new("dev".to_string(), Vec::new(), std::path::PathBuf::from("/tmp"));
    assert!(!spec.command.is_empty());
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test`

Expected: tests fail because PTY/server types are missing.

- [ ] **Step 3: Implement the minimal server**

Implement `server::run(socket_path)`:

1. Bind a Unix listener.
2. Accept one command per connection.
3. `NEW` creates a session with one PTY process.
4. PTY output is read on a background thread, appended to a bounded in-memory history, and broadcast to attached clients.
5. `CAPTURE` writes current history bytes to stdout response.
6. `LIST` returns one session name per line.
7. `KILL` sends SIGTERM to the session child and removes the session.
8. `KILL_SERVER` kills all sessions, removes the socket file, and exits the server process.

Use `forkpty` through local POSIX FFI in `src/pty.rs`; do not add external crates.

- [ ] **Step 4: Run all tests**

Run: `cargo test`

Expected: parser/unit tests pass and integration tests reach server behavior. If integration tests fail on timing, adjust the tests only by polling capture for up to 3 seconds; do not increase sleeps blindly.

- [ ] **Step 5: Commit server implementation**

Run:

```bash
git add src/main.rs src/pty.rs src/server.rs tests/phase1_cli.rs
git commit -m "feat: keep detached pty sessions on dmuxd"
```

---

### Task 4: Interactive Attach Passthrough

**Files:**
- Modify: `src/main.rs`
- Create: `src/client.rs`

- [ ] **Step 1: Add attach command contract test for missing sessions**

Add a CLI integration test:

```rust
#[test]
fn attach_reports_missing_session() {
    let socket = unique_socket("missing-attach");
    let output = dmux(&socket, &["attach", "-t", "missing"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing session"), "{stderr:?}");
    let _ = dmux(&socket, &["kill-server"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test phase1_cli attach_reports_missing_session`

Expected: failure or wrong error because attach client support is not complete.

- [ ] **Step 3: Implement attach**

Implement:

1. Client sends `ATTACH`.
2. Server replies `OK\n` or `ERR missing session\n`.
3. On success, server registers the connection for PTY output and forwards bytes received from the client into the PTY.
4. Client sets raw mode using `stty -g`, `stty raw -echo`, and restores it on exit.
5. Client copies socket output to stdout and stdin to socket.
6. `Ctrl-b d` in the client detaches without killing the PTY process.

- [ ] **Step 4: Run tests**

Run: `cargo test`

Expected: all automated tests pass.

- [ ] **Step 5: Commit attach implementation**

Run:

```bash
git add src/main.rs src/client.rs src/server.rs tests/phase1_cli.rs
git commit -m "feat: attach to live dmux pty sessions"
```

---

### Task 5: Verification And Documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Document the Phase 1 commands**

Update README with:

```markdown
# devmux

`dmux` is an early Rust terminal multiplexer prototype.

Implemented Phase 0/1 commands:

- `dmux new -d -s <name> [-- command...]`
- `dmux new -s <name> [-- command...]`
- `dmux attach -t <name>`
- `dmux ls`
- `dmux capture-pane -t <name> -p`
- `dmux kill-session -t <name>`
- `dmux kill-server`

Current limits:

- single pane per session
- in-memory scrollback only
- no layout/window support yet
- Unix/macOS POSIX PTY support only
```

- [ ] **Step 2: Run formatting and tests**

Run:

```bash
cargo fmt --check
cargo test
```

Expected: both commands pass.

- [ ] **Step 3: Commit docs**

Run:

```bash
git add README.md docs/superpowers/plans/2026-05-10-devmux-phase-0-1.md
git commit -m "docs: describe phase 1 dmux prototype"
```

---

## Self-Review

- Scope is limited to Phase 0/1 from `docs/spec_design.md`: Rust skeleton, CLI, server, Unix socket, single PTY shell/session, detach persistence, attach, capture, and cleanup.
- Phase 2 terminal parser, scrollback cells, layouts, copy mode, extended keys, OSC policy, agent hooks, and worktrees are intentionally excluded.
- The plan uses standard-library Rust plus POSIX FFI to avoid network-dependent dependencies during bootstrap.
