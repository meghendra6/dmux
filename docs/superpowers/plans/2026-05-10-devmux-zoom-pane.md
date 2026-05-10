# Zoom Pane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `zoom-pane` state so the active pane in a window can be toggled into a zoomed full-window state without removing other panes.

**Architecture:** Store zoom state on each `Window` as the zoomed pane index. Because this prototype does not render pane layouts yet, expose zoom state through `list-panes -F` format output; the default `list-panes` output remains unchanged for compatibility with existing tests and scripts. Selecting, removing, or explicitly zooming panes keeps active-pane and zoomed-pane indices valid.

**Tech Stack:** Rust standard library, existing CLI parser, existing Unix socket protocol, existing server `Window`/`PaneSet` state, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: CLI And Protocol Surface

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`

- [x] **Step 1: Write failing CLI tests**

Add tests in `src/cli.rs`:

```rust
#[test]
fn parses_zoom_pane_target_without_index() {
    let command = parse_args(["dmux", "zoom-pane", "-t", "dev"]).unwrap();
    assert_eq!(
        command,
        Command::ZoomPane {
            session: "dev".to_string(),
            pane: None,
        }
    );
}

#[test]
fn parses_zoom_pane_target_and_index() {
    let command = parse_args(["dmux", "zoom-pane", "-t", "dev", "-p", "0"]).unwrap();
    assert_eq!(
        command,
        Command::ZoomPane {
            session: "dev".to_string(),
            pane: Some(0),
        }
    );
}

#[test]
fn parses_list_panes_format() {
    let command = parse_args([
        "dmux",
        "list-panes",
        "-t",
        "dev",
        "-F",
        "#{pane.index}:#{pane.active}:#{pane.zoomed}",
    ])
    .unwrap();
    assert_eq!(
        command,
        Command::ListPanes {
            session: "dev".to_string(),
            format: Some("#{pane.index}:#{pane.active}:#{pane.zoomed}".to_string()),
        }
    );
}
```

- [x] **Step 2: Write failing protocol tests**

Add tests in `src/protocol.rs`:

```rust
#[test]
fn round_trips_zoom_pane_request_for_active_pane() {
    let line = encode_zoom_pane("dev", None);
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::ZoomPane {
            session: "dev".to_string(),
            pane: None,
        }
    );
}

#[test]
fn round_trips_zoom_pane_request_with_index() {
    let line = encode_zoom_pane("dev", Some(0));
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::ZoomPane {
            session: "dev".to_string(),
            pane: Some(0),
        }
    );
}

#[test]
fn round_trips_list_panes_format_request() {
    let line = encode_list_panes("dev", Some("#{pane.index}:#{pane.zoomed}"));
    assert_eq!(
        decode_request(&line).unwrap(),
        Request::ListPanes {
            session: "dev".to_string(),
            format: Some("#{pane.index}:#{pane.zoomed}".to_string()),
        }
    );
}
```

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test --bin dmux zoom_pane
cargo test --bin dmux list_panes_format
```

Expected: FAIL because `ZoomPane`, `encode_zoom_pane`, and list-pane formats do not exist yet.

- [x] **Step 4: Implement minimal CLI and protocol**

Make these code changes:

```rust
// src/cli.rs
Command::ListPanes {
    session: String,
    format: Option<String>,
}

Command::ZoomPane {
    session: String,
    pane: Option<usize>,
}
```

Parse `list-panes -t <session> [-F <format>]` and `zoom-pane -t <session> [-p <index>]`. Keep existing `list-panes -t <session>` behavior by setting `format: None`.

```rust
// src/protocol.rs
Request::ListPanes {
    session: String,
    format: Option<String>,
}

Request::ZoomPane {
    session: String,
    pane: Option<usize>,
}
```

Encode unformatted list requests as `LIST_PANES\t<session>\n`; encode formatted list requests as `LIST_PANES_FORMAT\t<session>\t<hex-utf8-format>\n`. Encode zoom requests as `ZOOM_PANE\t<session>\tactive\n` or `ZOOM_PANE\t<session>\t<index>\n`.

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test --bin dmux zoom_pane
cargo test --bin dmux list_panes_format
```

Expected: PASS.

### Task 2: Server Zoom State And Formatted Pane Listing

**Files:**
- Modify: `src/main.rs`
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [x] **Step 1: Write failing integration tests**

Add tests in `tests/phase1_cli.rs`:

```rust
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
    assert!(poll_capture(&socket, &session, "base-ready").contains("base-ready"));

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
    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0:0:0", "1:1:1"]);

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session]));
    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0:0:0", "1:1:0"]);

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
    assert!(poll_capture(&socket, &session, "base-ready").contains("base-ready"));

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

    assert_success(&dmux(&socket, &["zoom-pane", "-t", &session, "-p", "0"]));
    let panes = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.index}:#{pane.active}:#{pane.zoomed}",
        ],
    );
    assert_success(&panes);
    let panes = String::from_utf8_lossy(&panes.stdout);
    assert_eq!(panes.lines().collect::<Vec<_>>(), vec!["0:1:1", "1:0:0"]);

    let active = poll_capture(&socket, &session, "base-ready");
    assert!(active.contains("base-ready"), "{active:?}");
    assert!(!active.contains("split-ready"), "{active:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    assert_success(&dmux(&socket, &["kill-server"]));
}
```

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --test phase1_cli zoom_pane
```

Expected: FAIL because the server does not handle `zoom-pane`.

- [x] **Step 3: Implement server behavior**

Make these server changes:

```rust
struct Window {
    panes: PaneSet<Arc<Pane>>,
    zoomed: Option<usize>,
}
```

Add `Window::zoom_pane(target: Option<usize>) -> Result<(), String>`:

1. Resolve `target.unwrap_or(active_pane_index)`.
2. Return `missing pane` when the index is out of range.
3. If the resolved pane is already zoomed, set `zoomed = None`.
4. Otherwise select that pane and set `zoomed = Some(index)`.

Add `Window::pane_descriptions()` that returns one row per pane with `index`, `active`, and `zoomed`. Add `format_pane_line(format, row)` with these replacements:

```rust
"#{pane.index}" => row.index.to_string()
"#{pane.active}" => "1" when active else "0"
"#{pane.zoomed}" => "1" when zoomed else "0"
"#{window.zoomed_flag}" => "1" when any pane is zoomed else "0"
```

Update `select_pane`, `add_pane`, and `kill_pane` so zoom indices remain valid. `select_pane` should move the zoom to the newly selected pane when the window is already zoomed. `kill_pane` should clear zoom if the removed pane was zoomed, decrement the zoom index if a lower pane is removed, and leave it unchanged otherwise.

- [x] **Step 4: Verify GREEN**

Run:

```bash
cargo test --test phase1_cli zoom_pane
cargo test --bin dmux pane_set
```

Expected: PASS.

### Task 3: Docs, Verification, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-10-devmux-zoom-pane.md`

- [x] **Step 1: Document the command**

Add `dmux zoom-pane -t <name> [-p <index>]` and `dmux list-panes -t <name> [-F <format>]` to README. Mention zoom-state groundwork in implemented groundwork and leave the existing current limit that layout rendering is not implemented yet.

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
git add README.md src/cli.rs src/main.rs src/protocol.rs src/server.rs tests/phase1_cli.rs docs/superpowers/plans/2026-05-10-devmux-zoom-pane.md
git commit -m "feat: add pane zoom state"
git push -u origin devmux-zoom-pane
gh pr create --draft --base main --head devmux-zoom-pane --title "Add pane zoom state" --body "<summary and validation>"
```
