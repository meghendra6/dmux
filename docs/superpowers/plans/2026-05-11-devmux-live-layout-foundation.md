# Devmux Live Layout Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve split direction in server state and render multi-pane attach snapshots as deterministic split layouts.

**Architecture:** Add a small `LayoutNode` tree to each active window, update it when panes split or die, and use it only for multi-pane attach snapshots. Keep the existing live attach path unchanged for single visible panes and zoomed split windows.

**Tech Stack:** Rust standard library, existing Unix socket protocol, existing `src/server.rs` session/window/pane state, existing `TerminalState` screen capture, integration tests in `tests/phase1_cli.rs`.

---

### Task 1: Add Layout Tree Model

**Files:**
- Modify: `src/server.rs`

- [x] **Step 1: Write failing layout unit tests**

Add these tests to the existing `#[cfg(test)] mod tests` in `src/server.rs`:

```rust
#[test]
fn layout_node_splits_active_leaf_horizontally() {
    let mut layout = LayoutNode::Pane(0);

    assert!(layout.split_pane(0, SplitDirection::Horizontal, 1));

    assert_eq!(
        layout,
        LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        }
    );
}

#[test]
fn layout_node_removes_pane_and_shifts_remaining_indexes() {
    let mut layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        first: Box::new(LayoutNode::Pane(0)),
        second: Box::new(LayoutNode::Split {
            direction: SplitDirection::Vertical,
            first: Box::new(LayoutNode::Pane(1)),
            second: Box::new(LayoutNode::Pane(2)),
        }),
    };

    assert!(layout.remove_pane(1));

    assert_eq!(
        layout,
        LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            first: Box::new(LayoutNode::Pane(0)),
            second: Box::new(LayoutNode::Pane(1)),
        }
    );
}
```

- [x] **Step 2: Run test to verify RED**

Run:

```bash
cargo test server::tests::layout_node_splits_active_leaf_horizontally server::tests::layout_node_removes_pane_and_shifts_remaining_indexes
```

Expected: FAIL because `LayoutNode` and its methods do not exist yet.

- [x] **Step 3: Add the minimal layout tree implementation**

Update the import at the top of `src/server.rs`:

```rust
use crate::protocol::{self, BufferSelection, CaptureMode, Request, SplitDirection};
```

Add this type near `Window`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
enum LayoutNode {
    Pane(usize),
    Split {
        direction: SplitDirection,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    fn split_pane(
        &mut self,
        target: usize,
        direction: SplitDirection,
        new_index: usize,
    ) -> bool {
        match self {
            LayoutNode::Pane(index) if *index == target => {
                *self = LayoutNode::Split {
                    direction,
                    first: Box::new(LayoutNode::Pane(target)),
                    second: Box::new(LayoutNode::Pane(new_index)),
                };
                true
            }
            LayoutNode::Pane(_) => false,
            LayoutNode::Split { first, second, .. } => {
                first.split_pane(target, direction, new_index)
                    || second.split_pane(target, direction, new_index)
            }
        }
    }

    fn remove_pane(&mut self, removed: usize) -> bool {
        match self {
            LayoutNode::Pane(index) if *index == removed => false,
            LayoutNode::Pane(index) => {
                if *index > removed {
                    *index -= 1;
                }
                true
            }
            LayoutNode::Split { first, second, .. } => {
                let keep_first = first.remove_pane(removed);
                let keep_second = second.remove_pane(removed);
                match (keep_first, keep_second) {
                    (true, true) => true,
                    (true, false) => {
                        *self = (**first).clone();
                        true
                    }
                    (false, true) => {
                        *self = (**second).clone();
                        true
                    }
                    (false, false) => false,
                }
            }
        }
    }
}
```

- [x] **Step 4: Run test to verify GREEN**

Run:

```bash
cargo test server::tests::layout_node_splits_active_leaf_horizontally server::tests::layout_node_removes_pane_and_shifts_remaining_indexes
```

Expected: PASS.

- [ ] **Step 5: Commit layout model**

Run:

```bash
git add src/server.rs
git commit -m "feat: add pane layout tree"
```

### Task 2: Add Layout Renderer

**Files:**
- Modify: `src/server.rs`

- [ ] **Step 1: Write failing renderer unit tests**

Add these tests to `src/server.rs`:

```rust
#[test]
fn render_attach_layout_joins_horizontal_panes() {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Horizontal,
        first: Box::new(LayoutNode::Pane(0)),
        second: Box::new(LayoutNode::Pane(1)),
    };
    let panes = vec![
        PaneSnapshot {
            index: 0,
            screen: "base-ready\n".to_string(),
        },
        PaneSnapshot {
            index: 1,
            screen: "split-ready\n".to_string(),
        },
    ];

    let rendered = render_attach_pane_snapshot(&layout, &panes);

    assert!(rendered.contains("base-ready | split-ready\r\n"), "{rendered:?}");
}

#[test]
fn render_attach_layout_stacks_vertical_panes() {
    let layout = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        first: Box::new(LayoutNode::Pane(0)),
        second: Box::new(LayoutNode::Pane(1)),
    };
    let panes = vec![
        PaneSnapshot {
            index: 0,
            screen: "base-ready\n".to_string(),
        },
        PaneSnapshot {
            index: 1,
            screen: "split-ready\n".to_string(),
        },
    ];

    let rendered = render_attach_pane_snapshot(&layout, &panes);

    assert!(rendered.contains("base-ready\r\n-----------\r\nsplit-ready\r\n"), "{rendered:?}");
}
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test server::tests::render_attach_layout_joins_horizontal_panes server::tests::render_attach_layout_stacks_vertical_panes
```

Expected: FAIL because `render_attach_pane_snapshot` still accepts only a pane list and prints labeled sections.

- [ ] **Step 3: Implement renderer helpers**

Change `render_attach_pane_snapshot` to accept a layout:

```rust
fn render_attach_pane_snapshot(layout: &LayoutNode, panes: &[PaneSnapshot]) -> String {
    let screens = panes
        .iter()
        .map(|pane| (pane.index, pane.screen.as_str()))
        .collect::<HashMap<_, _>>();

    match render_layout_lines(layout, &screens) {
        Some(lines) => render_client_lines(&lines),
        None => render_ordered_pane_sections(panes),
    }
}
```

Add:

```rust
fn render_layout_lines(
    layout: &LayoutNode,
    screens: &HashMap<usize, &str>,
) -> Option<Vec<String>> {
    match layout {
        LayoutNode::Pane(index) => screens.get(index).map(|screen| screen_lines(screen)),
        LayoutNode::Split {
            direction,
            first,
            second,
        } => {
            let first = render_layout_lines(first, screens)?;
            let second = render_layout_lines(second, screens)?;
            Some(match direction {
                SplitDirection::Horizontal => join_horizontal(first, second),
                SplitDirection::Vertical => join_vertical(first, second),
            })
        }
    }
}

fn screen_lines(screen: &str) -> Vec<String> {
    let lines = screen.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn join_horizontal(left: Vec<String>, right: Vec<String>) -> Vec<String> {
    let width = max_line_width(&left);
    let rows = left.len().max(right.len());
    (0..rows)
        .map(|index| {
            let mut line = pad_to_width(left.get(index).map_or("", String::as_str), width);
            line.push_str(" | ");
            line.push_str(right.get(index).map_or("", String::as_str));
            line
        })
        .collect()
}

fn join_vertical(mut top: Vec<String>, bottom: Vec<String>) -> Vec<String> {
    let width = max_line_width(&top).max(max_line_width(&bottom)).max(1);
    top.push("-".repeat(width));
    top.extend(bottom);
    top
}

fn max_line_width(lines: &[String]) -> usize {
    lines.iter().map(|line| line.chars().count()).max().unwrap_or(0)
}

fn pad_to_width(line: &str, width: usize) -> String {
    let mut padded = line.to_string();
    let len = line.chars().count();
    if len < width {
        padded.push_str(&" ".repeat(width - len));
    }
    padded
}

fn render_client_lines(lines: &[String]) -> String {
    let mut output = String::new();
    for line in lines {
        output.push_str(line);
        output.push_str("\r\n");
    }
    output
}

fn render_ordered_pane_sections(panes: &[PaneSnapshot]) -> String {
    let mut output = String::new();
    for pane in panes {
        output.push_str("\r\n-- pane ");
        output.push_str(&pane.index.to_string());
        output.push_str(" --\r\n");
        for line in pane.screen.lines() {
            output.push_str(line);
            output.push_str("\r\n");
        }
    }
    output
}
```

- [ ] **Step 4: Run test to verify GREEN**

Run:

```bash
cargo test server::tests::render_attach_layout_joins_horizontal_panes server::tests::render_attach_layout_stacks_vertical_panes
```

Expected: PASS.

- [ ] **Step 5: Commit renderer**

Run:

```bash
git add src/server.rs
git commit -m "feat: render pane layout text"
```

### Task 3: Wire Split Layout Snapshots

**Files:**
- Modify: `src/server.rs`
- Modify: `tests/phase1_cli.rs`

- [ ] **Step 1: Write failing integration tests**

Modify `attach_renders_split_pane_snapshot` in `tests/phase1_cli.rs` to assert horizontal layout output:

```rust
assert_contains_ordered_line(&stdout, "base-ready", " | ", "split-ready");
assert!(!stdout.contains("-- pane 0 --"), "{stdout:?}");
assert!(!stdout.contains("-- pane 1 --"), "{stdout:?}");
```

Add this helper near other test helpers:

```rust
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
```

Add vertical split integration test:

```rust
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
```

Add helper:

```rust
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
```

- [ ] **Step 2: Run test to verify RED**

Run:

```bash
cargo test --test phase1_cli attach_renders_split_pane_snapshot attach_renders_vertical_split_layout_snapshot
```

Expected: FAIL because attach snapshots still use labeled sections and split direction is not stored in `Window`.

- [ ] **Step 3: Add attach layout snapshot data**

Add:

```rust
struct AttachLayoutSnapshot {
    layout: LayoutNode,
    panes: Vec<IndexedPane>,
}

impl AttachLayoutSnapshot {
    fn empty() -> Self {
        Self {
            layout: LayoutNode::Pane(0),
            panes: Vec::new(),
        }
    }
}
```

Add to `Session`:

```rust
fn attach_layout_snapshot(&self) -> AttachLayoutSnapshot {
    self.windows.lock().unwrap().attach_layout_snapshot()
}
```

Add to `WindowSet`:

```rust
fn attach_layout_snapshot(&self) -> AttachLayoutSnapshot {
    self.active_window()
        .map_or_else(AttachLayoutSnapshot::empty, Window::attach_layout_snapshot)
}
```

- [ ] **Step 4: Add layout to `Window` and pass split direction**

Change `Window`:

```rust
struct Window {
    panes: PaneSet<Arc<Pane>>,
    layout: LayoutNode,
    zoomed: Option<usize>,
}
```

Update `Window::new`:

```rust
fn new(pane: Arc<Pane>) -> Self {
    Self {
        panes: PaneSet::new(pane),
        layout: LayoutNode::Pane(0),
        zoomed: None,
    }
}
```

Update `Window::add_pane`:

```rust
fn add_pane(&mut self, direction: SplitDirection, pane: Arc<Pane>) {
    let split_index = self.panes.active_index();
    let new_index = self.panes.len();
    self.panes.add(pane);
    let _ = self.layout.split_pane(split_index, direction, new_index);
    if self.zoomed.is_some() {
        self.zoomed = Some(self.panes.active_index());
    }
}
```

Update `Window::kill_pane` after `self.panes.kill_at(index);`:

```rust
self.layout.remove_pane(index);
```

Add `Window::attach_layout_snapshot`:

```rust
fn attach_layout_snapshot(&self) -> AttachLayoutSnapshot {
    if let Some(index) = self.zoomed {
        let panes = self
            .panes
            .get(index)
            .cloned()
            .map(|pane| vec![IndexedPane { index, pane }])
            .unwrap_or_default();
        return AttachLayoutSnapshot {
            layout: LayoutNode::Pane(index),
            panes,
        };
    }

    AttachLayoutSnapshot {
        layout: self.layout.clone(),
        panes: (0..self.panes.len())
            .filter_map(|index| {
                self.panes
                    .get(index)
                    .cloned()
                    .map(|pane| IndexedPane { index, pane })
            })
            .collect(),
    }
}
```

Update `WindowSet::add_pane`:

```rust
fn add_pane(&mut self, direction: SplitDirection, pane: Arc<Pane>) {
    if let Some(window) = self.active_window_mut() {
        window.add_pane(direction, pane);
    }
}
```

Update `Session::add_pane`:

```rust
fn add_pane(&self, direction: SplitDirection, pane: Arc<Pane>) {
    self.windows.lock().unwrap().add_pane(direction, pane);
}
```

Update the request match:

```rust
Request::Split {
    session,
    direction,
    command,
} => handle_split(&state, &mut stream, &session, direction, command),
```

Update `handle_split` to accept `direction: SplitDirection` and call:

```rust
session.add_pane(direction, pane);
```

- [ ] **Step 5: Use layout in `attach_pane_snapshot`**

Update:

```rust
fn attach_pane_snapshot(session: &Session) -> Option<String> {
    let snapshot = session.attach_layout_snapshot();
    if snapshot.panes.len() <= 1 {
        return None;
    }

    let panes = snapshot
        .panes
        .into_iter()
        .map(|pane| PaneSnapshot {
            index: pane.index,
            screen: capture_pane_text(&pane.pane, CaptureMode::Screen),
        })
        .collect::<Vec<_>>();

    Some(render_attach_pane_snapshot(&snapshot.layout, &panes))
}
```

- [ ] **Step 6: Run integration tests to verify GREEN**

Run:

```bash
cargo test --test phase1_cli attach_renders_split_pane_snapshot attach_renders_vertical_split_layout_snapshot attach_keeps_zoomed_split_pane_live
```

Expected: PASS.

- [ ] **Step 7: Commit split snapshot wiring**

Run:

```bash
git add src/server.rs tests/phase1_cli.rs
git commit -m "feat: render split layout snapshots"
```

### Task 4: Documentation, Review, PR

**Files:**
- Modify: `README.md`
- Modify: `docs/superpowers/plans/2026-05-11-devmux-live-layout-foundation.md`

- [ ] **Step 1: Update README**

Update implemented groundwork from:

```text
- attach-time split-pane snapshot rendering
```

to:

```text
- attach-time split-pane layout snapshot rendering
```

Update current limits from:

```text
- multi-pane attach rendering is snapshot-only when multiple panes are visible and exits after rendering; live multi-pane attach, split-direction layout persistence, and live layout redraw are not implemented yet
```

to:

```text
- multi-pane attach rendering is split-layout snapshot-only when multiple panes are visible and exits after rendering; live multi-pane attach and live layout redraw are not implemented yet
```

- [ ] **Step 2: Run full verification**

Run:

```bash
cargo fmt --check
git diff --check origin/main
rg -ni "co""dex" .
cargo test
```

Expected: all commands exit 0; keyword scan prints no matches; unit and integration tests pass.

- [ ] **Step 3: Request critical review**

Dispatch a read-only review subagent against `git diff origin/main` with this brief:

```text
Review the live layout foundation diff for correctness. Focus on layout tree invariants, pane index shifting after kill-pane, split direction semantics, attach snapshot rendering, zoomed single-pane live attach behavior, and tests. Report only actionable correctness or maintainability issues.
```

Apply technically valid Critical or Important findings, rerun full verification, and update this plan's checkboxes.

- [ ] **Step 4: Open PR**

Run:

```bash
git push -u origin devmux-live-layout
gh pr create --draft --base main --head devmux-live-layout --title "Render split layout snapshots" --body "<summary and validation>"
gh pr ready
```

The PR title and body must not contain the banned project-assistant keyword.
