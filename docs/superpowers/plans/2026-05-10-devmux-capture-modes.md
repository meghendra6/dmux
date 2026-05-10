# Capture Modes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add explicit `capture-pane` modes for current screen, scrollback history, and combined output as the first Phase 4 copy/scrollback foundation.

**Architecture:** Keep the existing `capture-pane -p` behavior as combined scrollback plus screen output. Add a small `CaptureMode` enum shared by CLI/protocol/server, and add focused `TerminalState` methods for screen-only, history-only, and all capture. This slice does not add interactive copy mode or buffers yet.

**Tech Stack:** Rust standard library, existing CLI parser, existing Unix socket protocol, existing `TerminalState` scrollback/screen model, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Capture Modes

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI tests**

Add tests in `src/cli.rs`:

```rust
#[test]
fn parses_capture_pane_screen_mode() {
    let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p", "--screen"]).unwrap();
    assert_eq!(
        command,
        Command::CapturePane {
            session: "dev".to_string(),
            mode: CaptureMode::Screen,
        }
    );
}

#[test]
fn parses_capture_pane_history_mode() {
    let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p", "--history"]).unwrap();
    assert_eq!(
        command,
        Command::CapturePane {
            session: "dev".to_string(),
            mode: CaptureMode::History,
        }
    );
}

#[test]
fn parses_capture_pane_all_mode() {
    let command = parse_args(["dmux", "capture-pane", "-t", "dev", "-p", "--all"]).unwrap();
    assert_eq!(
        command,
        Command::CapturePane {
            session: "dev".to_string(),
            mode: CaptureMode::All,
        }
    );
}
```

- [x] **Step 2: Write failing protocol tests**

Add tests in `src/protocol.rs`:

```rust
#[test]
fn round_trips_capture_screen_request() {
    let line = encode_capture("dev", CaptureMode::Screen);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::Capture {
            session: "dev".to_string(),
            mode: CaptureMode::Screen,
        }
    );
}

#[test]
fn round_trips_capture_history_request() {
    let line = encode_capture("dev", CaptureMode::History);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::Capture {
            session: "dev".to_string(),
            mode: CaptureMode::History,
        }
    );
}
```

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test --bin dmux capture_pane_screen
cargo test --bin dmux capture_history
```

Expected: FAIL because `CaptureMode` does not exist yet.

- [x] **Step 4: Implement CLI and protocol**

Add `CaptureMode` to `src/protocol.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Screen,
    History,
    All,
}
```

Update `Command::CapturePane` and `Request::Capture` to include `mode: CaptureMode`. Parse `capture-pane -t <session> -p` as `All`, `--screen` as `Screen`, `--history` as `History`, and `--all` as `All`. Encode requests as `CAPTURE\t<session>\t<screen|history|all>\n`; decode the existing two-field `CAPTURE\t<session>\n` as `All` for compatibility.

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --bin dmux capture_pane_screen
cargo test --bin dmux capture_history
```

Expected: PASS.

### Task 2: TerminalState And Server Capture Modes

**Files:**
- Modify: `src/term.rs`
- Modify: `src/main.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing unit tests**

Add tests in `src/term.rs`:

```rust
#[test]
fn capture_screen_excludes_scrollback() {
    let mut state = TerminalState::new(20, 3, 100);
    state.apply_bytes(b"one\r\ntwo\r\nthree\r\nfour");
    let screen = state.capture_screen_text();
    assert_eq!(screen, "two\nthree\nfour\n");
    assert!(!screen.contains("one"), "{screen:?}");
    assert!(screen.contains("four"), "{screen:?}");
}

#[test]
fn capture_history_excludes_current_screen() {
    let mut state = TerminalState::new(20, 3, 100);
    state.apply_bytes(b"one\r\ntwo\r\nthree\r\nfour");
    let history = state.capture_history_text();
    assert_eq!(history, "one\n");
    assert!(history.contains("one"), "{history:?}");
    assert!(!history.contains("four"), "{history:?}");
}
```

- [x] **Step 2: Write failing integration test**

Add test in `tests/phase1_cli.rs`:

```rust
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

    let history = dmux(&socket, &["capture-pane", "-t", &session, "-p", "--history"]);
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

fn has_line(text: &str, needle: &str) -> bool {
    text.lines().any(|line| line == needle)
}
```

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test --bin dmux capture_screen
cargo test --test phase1_cli capture_pane_modes
```

Expected: FAIL because terminal/server capture mode handling is not implemented.

- [x] **Step 4: Implement terminal and server behavior**

Add `TerminalState::capture_screen_text`, `TerminalState::capture_history_text`, and make `capture_text` call the new combined helper. Update `handle_capture` to select the right method based on `CaptureMode`. Update `src/main.rs` to call `protocol::encode_capture(&session, mode)`.

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --bin dmux capture_screen
cargo test --test phase1_cli capture_pane_modes
```

Expected: PASS.

### Task 3: Docs, Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-capture-modes.md`

- [x] **Step 1: Document capture modes**

Update README command list to show:

```text
dmux capture-pane -t <name> -p [--screen|--history|--all]
```

Mention explicit screen/history/all capture modes in the implemented groundwork.

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

- [x] **Step 4: Commit and open draft PR**

Run:

```bash
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs src/term.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-capture-modes.md
git commit -m "feat: add capture pane modes"
git push -u origin devmux-capture-modes
gh pr create --draft --base main --head devmux-capture-modes --title "Add capture pane modes" --body "<summary and validation>"
```
