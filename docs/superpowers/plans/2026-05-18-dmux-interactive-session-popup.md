# dmux Interactive Session Popup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an interactive modal popup navigator for dmux so rows can be selected, peeked, and focused or attached from the current terminal.

**Architecture:** Add a focused client-side popup model in `src/popup.rs`, keep the server as the source of mux/session facts, and wire the model into both composed/live attach and raw attach. Existing `C-b !`, `C-b A`, and `C-b w` become scoped entry points into the same interaction framework.

**Tech Stack:** Rust standard library, existing dmux Unix socket protocol, existing `src/client.rs` attach/render paths, existing `src/registry.rs` local registry, integration tests in `tests/phase1_cli.rs`.

---

## Source Documents

- Spec: `docs/superpowers/specs/2026-05-18-dmux-interactive-session-popup-design.md`
- Current client popup code: `src/client.rs`
- Current registry store: `src/registry.rs`
- Current CLI/protocol agent event path: `src/cli.rs`, `src/main.rs`, `src/protocol.rs`, `src/server.rs`
- Existing integration tests: `tests/phase1_cli.rs`

## Scope Notes

This plan implements the interactive control surface through focus, cross-session
switch, read-only peek, registry hardening, and a normalized agent event schema.

The following remain future plans after this one is stable:

- reply from peek
- dispatch new background sessions
- pin and reorder rows
- rename rows or sessions
- stop/delete with confirmation
- PR/check status dots
- worktree create/open/list/remove
- desktop notification and clipboard relay

## File Structure

- Create `src/popup.rs`
  - Owns pure popup data structures and behavior: row model, selection, scrolling, grouping, filtering, peek state, state normalization, and text rendering.
  - Contains unit tests for all model behavior.
- Modify `src/main.rs`
  - Add `mod popup;`.
- Modify `src/client.rs`
  - Convert existing popup enum usage to the new model.
  - Build popup rows from live mux state, registry state, and agent events.
  - Route popup-mode keys before child PTY forwarding.
  - Execute `Enter` focus/attach/switch behavior.
  - Render popup and read-only peek overlays.
- Modify `src/registry.rs`
  - Add conservative metadata required for previous/stale rows.
  - Preserve forward migration for the current v1 registry file format.
- Modify `src/cli.rs`
  - Extend `agent-event` parsing only after the event schema task.
  - Keep backward compatibility with the current `agent-event -t <target> --state <state> [--label <text>]`.
- Modify `src/main.rs`, `src/protocol.rs`, `src/server.rs`
  - Extend the agent event wire format only after popup interaction works.
- Modify `tests/phase1_cli.rs`
  - Add integration tests for popup key handling, row navigation, focus, cross-session attach, disabled previous rows, and peek.
- Modify `README.md`
  - Document interactive popup behavior after the feature is implemented.

## Milestone Order

1. Interactive popup foundation with pure model tests.
2. Current-session `C-b w` tree navigator and same-session focus.
3. Workspace `C-b A` navigator and cross-session switch.
4. Attention `C-b !` navigator and read-only peek.
5. Registry hardening for previous/stale rows.
6. Normalized agent event schema.
7. README update and full verification.

Each milestone should be a separate commit.

---

### Task 1: Add Pure Popup Model

**Files:**
- Create: `src/popup.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Add the module declaration**

In `src/main.rs`, add the module next to the existing module declarations:

```rust
mod popup;
```

- [ ] **Step 2: Create the popup data model**

Create `src/popup.rs` with these public types and helpers:

```rust
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupMode {
    Attention,
    Workspace,
    Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupGrouping {
    Attention,
    Repo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PopupStateKind {
    NeedsInput,
    Alert,
    Ready,
    Failed,
    Working,
    Completed,
    Idle,
    Detached,
    Previous,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupRowKind {
    Header,
    Item,
    DisabledItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopupRowSource {
    Mux,
    Registry,
    AgentEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupTarget {
    pub session: String,
    pub window_index: Option<usize>,
    pub pane_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupRow {
    pub id: String,
    pub kind: PopupRowKind,
    pub repo_path: Option<PathBuf>,
    pub target: Option<PopupTarget>,
    pub state: PopupStateKind,
    pub source: PopupRowSource,
    pub title: String,
    pub summary: String,
    pub last_changed: Option<u64>,
    pub attachable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupModel {
    pub rows: Vec<PopupRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupState {
    pub mode: PopupMode,
    pub grouping: PopupGrouping,
    pub selected: Option<String>,
    pub scroll: usize,
    pub filter: String,
    pub filter_mode: bool,
    pub peek: bool,
}
```

- [ ] **Step 3: Add constructors and selection helpers**

Add this implementation in `src/popup.rs`:

```rust
impl PopupState {
    pub fn new(mode: PopupMode) -> Self {
        Self {
            mode,
            grouping: PopupGrouping::Attention,
            selected: None,
            scroll: 0,
            filter: String::new(),
            filter_mode: false,
            peek: false,
        }
    }

    pub fn toggle_grouping(&mut self) {
        self.grouping = match self.grouping {
            PopupGrouping::Attention => PopupGrouping::Repo,
            PopupGrouping::Repo => PopupGrouping::Attention,
        };
    }

    pub fn close_or_clear(&mut self) -> PopupCloseResult {
        if self.peek {
            self.peek = false;
            PopupCloseResult::StayOpen
        } else if self.filter_mode {
            self.filter_mode = false;
            self.filter.clear();
            PopupCloseResult::StayOpen
        } else if !self.filter.is_empty() {
            self.filter.clear();
            PopupCloseResult::StayOpen
        } else {
            PopupCloseResult::Close
        }
    }

    pub fn ensure_selection(&mut self, model: &PopupModel) {
        if let Some(selected) = &self.selected {
            if model
                .rows
                .iter()
                .any(|row| row.id == *selected && row.kind != PopupRowKind::Header)
            {
                return;
            }
        }
        self.selected = model
            .rows
            .iter()
            .find(|row| row.kind != PopupRowKind::Header)
            .map(|row| row.id.clone());
    }

    pub fn move_selection(&mut self, model: &PopupModel, delta: isize) {
        let selectable = model.selectable_row_ids();
        if selectable.is_empty() {
            self.selected = None;
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|id| selectable.iter().position(|candidate| candidate == id))
            .unwrap_or(0);
        let next = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current
                .saturating_add(delta as usize)
                .min(selectable.len().saturating_sub(1))
        };
        self.selected = Some(selectable[next].clone());
    }

    pub fn selected_row<'a>(&self, model: &'a PopupModel) -> Option<&'a PopupRow> {
        let selected = self.selected.as_ref()?;
        model.rows.iter().find(|row| row.id == *selected)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupCloseResult {
    StayOpen,
    Close,
}

impl PopupModel {
    pub fn new(rows: Vec<PopupRow>) -> Self {
        Self { rows }
    }

    pub fn selectable_row_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.kind != PopupRowKind::Header)
            .map(|row| row.id.clone())
            .collect()
    }
}
```

- [ ] **Step 4: Add row filtering**

Add this helper in `src/popup.rs`:

```rust
pub fn filter_rows(rows: &[PopupRow], filter: &str) -> Vec<PopupRow> {
    let needle = filter.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return rows.to_vec();
    }

    rows.iter()
        .filter(|row| {
            if row.kind == PopupRowKind::Header {
                return true;
            }
            let repo = row
                .repo_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            let target = row
                .target
                .as_ref()
                .map(|target| {
                    format!(
                        "{} {} {}",
                        target.session,
                        target
                            .window_index
                            .map(|index| index.to_string())
                            .unwrap_or_default(),
                        target
                            .pane_index
                            .map(|index| index.to_string())
                            .unwrap_or_default()
                    )
                })
                .unwrap_or_default();
            format!(
                "{} {} {} {} {:?}",
                repo, target, row.title, row.summary, row.state
            )
            .to_ascii_lowercase()
            .contains(&needle)
        })
        .cloned()
        .collect()
}
```

- [ ] **Step 5: Add unit tests for selection and filtering**

Add this test module at the end of `src/popup.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, title: &str, kind: PopupRowKind) -> PopupRow {
        PopupRow {
            id: id.to_string(),
            kind,
            repo_path: None,
            target: Some(PopupTarget {
                session: "dev".to_string(),
                window_index: Some(0),
                pane_index: Some(0),
            }),
            state: PopupStateKind::Working,
            source: PopupRowSource::Mux,
            title: title.to_string(),
            summary: String::new(),
            last_changed: None,
            attachable: kind == PopupRowKind::Item,
        }
    }

    #[test]
    fn selection_skips_headers() {
        let model = PopupModel::new(vec![
            row("h", "Working", PopupRowKind::Header),
            row("a", "alpha", PopupRowKind::Item),
            row("b", "beta", PopupRowKind::Item),
        ]);
        let mut state = PopupState::new(PopupMode::Tree);

        state.ensure_selection(&model);
        assert_eq!(state.selected.as_deref(), Some("a"));

        state.move_selection(&model, 1);
        assert_eq!(state.selected.as_deref(), Some("b"));

        state.move_selection(&model, -1);
        assert_eq!(state.selected.as_deref(), Some("a"));
    }

    #[test]
    fn filtering_keeps_headers_and_matching_items() {
        let rows = vec![
            row("h", "Working", PopupRowKind::Header),
            row("a", "alpha api", PopupRowKind::Item),
            row("b", "beta ui", PopupRowKind::Item),
        ];

        let filtered = filter_rows(&rows, "api");
        assert_eq!(
            filtered.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec!["h", "a"]
        );
    }

    #[test]
    fn escape_closes_peek_before_popup() {
        let mut state = PopupState::new(PopupMode::Attention);
        state.peek = true;

        assert_eq!(state.close_or_clear(), PopupCloseResult::StayOpen);
        assert!(!state.peek);
        assert_eq!(state.close_or_clear(), PopupCloseResult::Close);
    }
}
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test popup -- --test-threads=1
```

Expected: all `popup` unit tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/popup.rs
git commit -m "feat: add interactive popup model"
```

---

### Task 2: Add Popup Key Actions Without Changing Visible Behavior

**Files:**
- Modify: `src/client.rs`
- Test: `src/client.rs` unit tests

- [ ] **Step 1: Write key translation tests**

In the existing `#[cfg(test)]` module in `src/client.rs`, add tests near the
existing attach input translation tests:

```rust
#[test]
fn live_snapshot_popup_mode_consumes_navigation_keys() {
    let mut state = PopupInputState::new(crate::popup::PopupMode::Attention);

    assert_eq!(
        popup_actions_for_input(b"j", &mut state),
        vec![PopupInputAction::MoveDown]
    );
    assert_eq!(
        popup_actions_for_input(b"k", &mut state),
        vec![PopupInputAction::MoveUp]
    );
    assert_eq!(
        popup_actions_for_input(b"\r", &mut state),
        vec![PopupInputAction::Enter]
    );
    assert_eq!(
        popup_actions_for_input(b" ", &mut state),
        vec![PopupInputAction::TogglePeek]
    );
}

#[test]
fn popup_filter_mode_collects_text() {
    let mut state = PopupInputState::new(crate::popup::PopupMode::Workspace);

    assert_eq!(
        popup_actions_for_input(b"/api\x7f2\r", &mut state),
        vec![
            PopupInputAction::FilterStart,
            PopupInputAction::FilterPush('a'),
            PopupInputAction::FilterPush('p'),
            PopupInputAction::FilterPush('i'),
            PopupInputAction::FilterBackspace,
            PopupInputAction::FilterPush('2'),
            PopupInputAction::FilterAccept,
        ]
    );
}
```

Expected red result: `PopupInputState`, `PopupInputAction`, and
`popup_actions_for_input` do not exist.

- [ ] **Step 2: Add popup input types**

Add these types near the existing attach input state types in `src/client.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct PopupInputState {
    mode: crate::popup::PopupMode,
    filter_mode: bool,
}

impl PopupInputState {
    fn new(mode: crate::popup::PopupMode) -> Self {
        Self {
            mode,
            filter_mode: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PopupInputAction {
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    TogglePeek,
    ToggleGrouping,
    FilterStart,
    FilterPush(char),
    FilterBackspace,
    FilterAccept,
    Escape,
    Close,
}
```

- [ ] **Step 3: Add popup input decoder**

Add this helper near `translate_attach_input_with_state_with_controls`:

```rust
fn popup_actions_for_input(input: &[u8], state: &mut PopupInputState) -> Vec<PopupInputAction> {
    let mut actions = Vec::new();
    let mut index = 0;
    while index < input.len() {
        match input[index] {
            b'/' if !state.filter_mode => {
                state.filter_mode = true;
                actions.push(PopupInputAction::FilterStart);
                index += 1;
            }
            b'\r' | b'\n' if state.filter_mode => {
                state.filter_mode = false;
                actions.push(PopupInputAction::FilterAccept);
                index += 1;
            }
            b'\r' | b'\n' => {
                actions.push(PopupInputAction::Enter);
                index += 1;
            }
            b'\x1b' => {
                if input.get(index..index + 3) == Some(b"\x1b[A") {
                    actions.push(PopupInputAction::MoveUp);
                    index += 3;
                } else if input.get(index..index + 3) == Some(b"\x1b[B") {
                    actions.push(PopupInputAction::MoveDown);
                    index += 3;
                } else if input.get(index..index + 3) == Some(b"\x1b[C") {
                    actions.push(PopupInputAction::Enter);
                    index += 3;
                } else {
                    actions.push(PopupInputAction::Escape);
                    index += 1;
                }
            }
            b'\t' if !state.filter_mode => {
                actions.push(PopupInputAction::ToggleGrouping);
                index += 1;
            }
            b' ' if !state.filter_mode => {
                actions.push(PopupInputAction::TogglePeek);
                index += 1;
            }
            b'j' if !state.filter_mode => {
                actions.push(PopupInputAction::MoveDown);
                index += 1;
            }
            b'k' if !state.filter_mode => {
                actions.push(PopupInputAction::MoveUp);
                index += 1;
            }
            b'q' if !state.filter_mode => {
                actions.push(PopupInputAction::Close);
                index += 1;
            }
            b'\x7f' if state.filter_mode => {
                actions.push(PopupInputAction::FilterBackspace);
                index += 1;
            }
            byte if state.filter_mode && byte.is_ascii_graphic() || byte == b' ' => {
                actions.push(PopupInputAction::FilterPush(byte as char));
                index += 1;
            }
            _ => {
                index += 1;
            }
        }
    }
    actions
}
```

- [ ] **Step 4: Run the focused tests**

Run:

```bash
cargo test popup_mode -- --test-threads=1
cargo test popup_filter_mode -- --test-threads=1
```

Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/client.rs
git commit -m "feat: decode interactive popup input"
```

---

### Task 3: Render Existing Popup Content Through Popup Model

**Files:**
- Modify: `src/popup.rs`
- Modify: `src/client.rs`
- Test: `src/popup.rs`, `src/client.rs`

- [ ] **Step 1: Add render tests for selectable rows**

Add to `src/popup.rs` tests:

```rust
#[test]
fn render_marks_selected_row_and_footer() {
    let model = PopupModel::new(vec![
        row("h", "Working", PopupRowKind::Header),
        row("a", "alpha", PopupRowKind::Item),
    ]);
    let mut state = PopupState::new(PopupMode::Tree);
    state.selected = Some("a".to_string());

    let text = render_popup_text(&state, &model, None);

    assert!(text.contains("Working"));
    assert!(text.contains("> alpha"));
    assert!(text.contains("Enter: focus/attach"));
}
```

Expected red result: `render_popup_text` does not exist.

- [ ] **Step 2: Implement popup text rendering**

Add to `src/popup.rs`:

```rust
pub fn render_popup_text(state: &PopupState, model: &PopupModel, peek: Option<&str>) -> String {
    let mut lines = Vec::new();
    for row in &model.rows {
        match row.kind {
            PopupRowKind::Header => lines.push(row.title.clone()),
            PopupRowKind::Item | PopupRowKind::DisabledItem => {
                let selected = state.selected.as_deref() == Some(row.id.as_str());
                let marker = if selected { ">" } else { " " };
                let state_marker = state_marker(row.state);
                let disabled = if row.kind == PopupRowKind::DisabledItem {
                    " disabled"
                } else {
                    ""
                };
                lines.push(format!(
                    "{} {} {:<10} {}{}",
                    marker, state_marker, row.title, row.summary, disabled
                ));
            }
        }
    }
    if let Some(peek) = peek {
        lines.push(String::new());
        lines.push("Peek".to_string());
        lines.extend(peek.lines().map(str::to_string));
    }
    lines.push(String::new());
    lines.push("Enter: focus/attach   Space: peek   Tab: group   /: filter   Esc: close".to_string());
    lines.join("\n")
}

pub fn state_marker(state: PopupStateKind) -> &'static str {
    match state {
        PopupStateKind::NeedsInput => "!",
        PopupStateKind::Alert => "!",
        PopupStateKind::Ready => "+",
        PopupStateKind::Failed => "x",
        PopupStateKind::Working => "*",
        PopupStateKind::Completed => "+",
        PopupStateKind::Idle => "-",
        PopupStateKind::Detached => ".",
        PopupStateKind::Previous => ".",
        PopupStateKind::Stale => "x",
    }
}
```

- [ ] **Step 3: Add scoped legacy model builder**

In `src/client.rs`, add a helper that turns current legacy text into a disabled
model. This keeps visible behavior stable while the model is introduced:

```rust
fn legacy_popup_model(content: &str, mode: crate::popup::PopupMode) -> crate::popup::PopupModel {
    let rows = content
        .lines()
        .enumerate()
        .map(|(index, line)| crate::popup::PopupRow {
            id: format!("legacy-{index}"),
            kind: crate::popup::PopupRowKind::DisabledItem,
            repo_path: None,
            target: None,
            state: crate::popup::PopupStateKind::Idle,
            source: crate::popup::PopupRowSource::Mux,
            title: line.to_string(),
            summary: String::new(),
            last_changed: None,
            attachable: false,
        })
        .collect();
    let _ = mode;
    crate::popup::PopupModel::new(rows)
}
```

- [ ] **Step 4: Route existing `attach_popup_overlay_text` through the model**

Update `attach_popup_overlay_text` so `Attention`, `Tree`, and `WorkspaceRegistry`
still produce the same information but include the common footer from
`render_popup_text`. The title remains unchanged:

```rust
let content = attach_attention_overlay_text(socket, session)?;
let model = legacy_popup_model(&content, crate::popup::PopupMode::Attention);
let mut state = crate::popup::PopupState::new(crate::popup::PopupMode::Attention);
state.ensure_selection(&model);
Ok(Some(PopupOverlayText {
    title: "dmux attention",
    content: crate::popup::render_popup_text(&state, &model, None),
}))
```

Apply the same shape for `Tree` and `WorkspaceRegistry`. Leave `Help` and
`Detail` unchanged for now because they are not navigators.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test render_marks_selected_row_and_footer -- --test-threads=1
cargo test attach_input_shows_tree_detail_and_workspace_popups -- --test-threads=1
```

Expected: tests pass. If the existing snapshot-style assertions expect exact
legacy text, update them to assert the important content plus the shared footer.

- [ ] **Step 6: Commit**

```bash
git add src/popup.rs src/client.rs tests/phase1_cli.rs
git commit -m "feat: render popups through shared model"
```

---

### Task 4: Implement Current-Session Tree Navigation

**Files:**
- Modify: `src/client.rs`
- Modify: `src/popup.rs`
- Test: `src/client.rs`, `tests/phase1_cli.rs`

- [ ] **Step 1: Add a row builder test for tree rows**

Add this unit test in `src/client.rs` near tree popup tests:

```rust
#[test]
fn tree_popup_rows_include_window_and_pane_targets() {
    let windows = vec![WindowTreeEntry {
        index: 0,
        active: true,
        name: "main".to_string(),
        panes: vec![
            TreePaneEntry {
                index: 0,
                active: true,
                state: "running".to_string(),
                bell: false,
                activity: false,
                agent_state: String::new(),
                agent_label: String::new(),
                title: "shell".to_string(),
                cwd: "/tmp/project".to_string(),
            },
            TreePaneEntry {
                index: 1,
                active: false,
                state: "running".to_string(),
                bell: true,
                activity: false,
                agent_state: String::new(),
                agent_label: String::new(),
                title: "tests".to_string(),
                cwd: "/tmp/project".to_string(),
            },
        ],
    }];

    let model = tree_popup_model("dev", &windows);

    assert!(model.rows.iter().any(|row| row.kind == crate::popup::PopupRowKind::Header));
    let pane = model
        .rows
        .iter()
        .find(|row| row.target.as_ref().and_then(|target| target.pane_index) == Some(1))
        .expect("pane row");
    assert_eq!(pane.state, crate::popup::PopupStateKind::Alert);
    assert!(pane.attachable);
}
```

Expected red result: `tree_popup_model` does not exist.

- [ ] **Step 2: Implement tree row builder**

Add this helper near `format_session_tree_popup`:

```rust
fn tree_popup_model(session: &str, windows: &[WindowTreeEntry]) -> crate::popup::PopupModel {
    let mut rows = Vec::new();
    for window in windows {
        rows.push(crate::popup::PopupRow {
            id: format!("window-{}", window.index),
            kind: crate::popup::PopupRowKind::Header,
            repo_path: None,
            target: None,
            state: crate::popup::PopupStateKind::Idle,
            source: crate::popup::PopupRowSource::Mux,
            title: format!("Window {} {}", window.index, window.name),
            summary: String::new(),
            last_changed: None,
            attachable: false,
        });
        for pane in &window.panes {
            let state = if !pane.agent_state.is_empty() {
                normalize_agent_state(&pane.agent_state)
            } else if pane.bell || pane.activity {
                crate::popup::PopupStateKind::Alert
            } else if pane.state == "exited" {
                crate::popup::PopupStateKind::Completed
            } else {
                crate::popup::PopupStateKind::Working
            };
            rows.push(crate::popup::PopupRow {
                id: format!("{}:{}:{}", session, window.index, pane.index),
                kind: crate::popup::PopupRowKind::Item,
                repo_path: Some(PathBuf::from(&pane.cwd)),
                target: Some(crate::popup::PopupTarget {
                    session: session.to_string(),
                    window_index: Some(window.index),
                    pane_index: Some(pane.index),
                }),
                state,
                source: crate::popup::PopupRowSource::Mux,
                title: format!("pane {} {}", pane.index, pane.title),
                summary: pane.agent_label.clone(),
                last_changed: None,
                attachable: true,
            });
        }
    }
    crate::popup::PopupModel::new(rows)
}
```

Add this state mapper near the helper:

```rust
fn normalize_agent_state(state: &str) -> crate::popup::PopupStateKind {
    match state {
        "needs_input" | "waiting" | "permission" => crate::popup::PopupStateKind::NeedsInput,
        "ready" | "review" => crate::popup::PopupStateKind::Ready,
        "failed" | "error" => crate::popup::PopupStateKind::Failed,
        "completed" | "done" => crate::popup::PopupStateKind::Completed,
        "alert" => crate::popup::PopupStateKind::Alert,
        "working" | "running" => crate::popup::PopupStateKind::Working,
        _ => crate::popup::PopupStateKind::Idle,
    }
}
```

- [ ] **Step 3: Add popup enter helper for same-session targets**

Add this helper near `select_numbered_pane`:

```rust
fn perform_popup_enter_same_session(
    socket: &Path,
    current_session: &str,
    row: &crate::popup::PopupRow,
) -> io::Result<PopupEnterResult> {
    let Some(target) = row.target.as_ref() else {
        return Ok(PopupEnterResult::Message("row is not attachable".to_string()));
    };
    if !row.attachable || target.session != current_session {
        return Ok(PopupEnterResult::Message("row is not attachable here".to_string()));
    }
    if let Some(window_index) = target.window_index {
        let _ = send_control_request(
            socket,
            &protocol::encode_select_window(current_session, window_index),
        )?;
    }
    if let Some(pane_index) = target.pane_index {
        let _ = send_control_request(socket, &protocol::encode_select_pane(current_session, pane_index))?;
    }
    Ok(PopupEnterResult::Reconnect)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PopupEnterResult {
    Reconnect,
    Message(String),
}
```

- [ ] **Step 4: Wire `Enter` for `C-b w` in composed attach**

In `run_live_snapshot_attach`, replace `active_popup: AttachPopup` with a state
holder:

```rust
let mut active_popup = AttachPopup::None;
let mut popup_state: Option<crate::popup::PopupState> = None;
let mut popup_input_state: Option<PopupInputState> = None;
```

When handling `ShowTree`, initialize:

```rust
active_popup = active_popup.toggle_tree();
popup_state = if active_popup == AttachPopup::Tree {
    Some(crate::popup::PopupState::new(crate::popup::PopupMode::Tree))
} else {
    None
};
popup_input_state = if active_popup == AttachPopup::Tree {
    Some(PopupInputState::new(crate::popup::PopupMode::Tree))
} else {
    None
};
```

Before forwarding normal `LiveSnapshotInputEvent::Forward(bytes)`, if
`popup_input_state.is_some()`, decode with `popup_actions_for_input`, apply
movement or enter, redraw, and do not call `forward_live_snapshot_input`.

- [ ] **Step 5: Wire `Enter` for `C-b w` in raw attach**

In `forward_stdin_until_detach`, add the same `popup_state` and
`popup_input_state` values. Before processing normal translated attach actions,
if a popup is active and raw bytes arrive, handle popup navigation first and do
not forward those bytes to `stream`.

For `PopupEnterResult::Reconnect`, return:

```rust
return Ok(RawAttachExit::Reconnect {
    pending_input: raw_pending_input(
        &actions[index + 1..],
        input_state.saw_prefix,
        RawPendingFocus::Preserve,
        &controls,
    ),
});
```

- [ ] **Step 6: Add integration test for tree focus**

Add to `tests/phase1_cli.rs`:

```rust
#[test]
fn interactive_tree_popup_enter_selects_pane_for_input() {
    let socket = unique_socket("interactive-tree-enter");
    let session = format!("interactive-tree-enter-{}", std::process::id());
    assert_success(&dmux(&socket, &["new", "-d", "-s", &session, "--", "sh", "-lc", "cat"]));
    assert_success(&dmux(&socket, &["split-window", "-t", &session, "-h", "--", "sh", "-lc", "cat"]));

    let mut child = spawn_attached_to_session(&socket, &session, &[]);
    child.stdin_mut("tree popup stdin").write_all(b"\x02w").unwrap();
    child.wait_for_stdout_contains_all(&["Enter: focus/attach"], "tree popup");
    child.stdin_mut("tree popup stdin").write_all(b"j\rselected-pane\n").unwrap();

    let captured = poll_capture(&socket, &session, "selected-pane");
    assert!(captured.contains("selected-pane"), "{captured:?}");

    child.stdin_mut("tree popup stdin").write_all(b"\x02d").unwrap();
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
}
```

- [ ] **Step 7: Run focused tests**

```bash
cargo test tree_popup_rows_include_window_and_pane_targets -- --test-threads=1
cargo test interactive_tree_popup_enter_selects_pane_for_input -- --test-threads=1
```

Expected: tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/client.rs src/popup.rs tests/phase1_cli.rs
git commit -m "feat: navigate current session tree popup"
```

---

### Task 5: Implement Workspace Navigator And Cross-Session Switch

**Files:**
- Modify: `src/client.rs`
- Modify: `src/popup.rs`
- Test: `tests/phase1_cli.rs`

- [ ] **Step 1: Add workspace row model test**

Add to `src/client.rs` tests:

```rust
#[test]
fn workspace_popup_model_marks_previous_records_disabled() {
    let registry = crate::registry::WorkspaceRegistry {
        workspaces: vec![crate::registry::WorkspaceRecord {
            path: PathBuf::from("/tmp/repo"),
        }],
        sessions: vec![crate::registry::SessionRecord {
            name: "old".to_string(),
            path: PathBuf::from("/tmp/repo"),
            state: "stopped".to_string(),
            last_seen: 10,
        }],
    };
    let live = vec![LiveWorkspaceSession {
        name: "live".to_string(),
        window_count: 1,
        attached_count: 0,
    }];

    let model = workspace_popup_model(&registry, &live);

    let previous = model.rows.iter().find(|row| row.title.contains("old")).unwrap();
    assert_eq!(previous.kind, crate::popup::PopupRowKind::DisabledItem);
    assert!(!previous.attachable);

    let live = model.rows.iter().find(|row| row.title.contains("live")).unwrap();
    assert_eq!(live.kind, crate::popup::PopupRowKind::Item);
    assert!(live.attachable);
}
```

Expected red result: `workspace_popup_model` does not exist.

- [ ] **Step 2: Implement workspace row model**

Add near `format_workspace_registry_popup`:

```rust
fn workspace_popup_model(
    registry: &crate::registry::WorkspaceRegistry,
    live_sessions: &[LiveWorkspaceSession],
) -> crate::popup::PopupModel {
    let mut rows = Vec::new();
    rows.push(crate::popup::PopupRow {
        id: "group-live".to_string(),
        kind: crate::popup::PopupRowKind::Header,
        repo_path: None,
        target: None,
        state: crate::popup::PopupStateKind::Idle,
        source: crate::popup::PopupRowSource::Mux,
        title: "Live sessions".to_string(),
        summary: String::new(),
        last_changed: None,
        attachable: false,
    });
    for session in live_sessions {
        rows.push(crate::popup::PopupRow {
            id: format!("live:{}", session.name),
            kind: crate::popup::PopupRowKind::Item,
            repo_path: registry
                .sessions
                .iter()
                .find(|record| record.name == session.name)
                .map(|record| record.path.clone()),
            target: Some(crate::popup::PopupTarget {
                session: session.name.clone(),
                window_index: None,
                pane_index: None,
            }),
            state: if session.attached_count > 0 {
                crate::popup::PopupStateKind::Idle
            } else {
                crate::popup::PopupStateKind::Detached
            },
            source: crate::popup::PopupRowSource::Mux,
            title: session.name.clone(),
            summary: format!("windows {} clients {}", session.window_count, session.attached_count),
            last_changed: None,
            attachable: true,
        });
    }
    rows.push(crate::popup::PopupRow {
        id: "group-previous".to_string(),
        kind: crate::popup::PopupRowKind::Header,
        repo_path: None,
        target: None,
        state: crate::popup::PopupStateKind::Previous,
        source: crate::popup::PopupRowSource::Registry,
        title: "Previous sessions".to_string(),
        summary: String::new(),
        last_changed: None,
        attachable: false,
    });
    for record in &registry.sessions {
        if live_sessions.iter().any(|session| session.name == record.name) {
            continue;
        }
        rows.push(crate::popup::PopupRow {
            id: format!("previous:{}", record.name),
            kind: crate::popup::PopupRowKind::DisabledItem,
            repo_path: Some(record.path.clone()),
            target: Some(crate::popup::PopupTarget {
                session: record.name.clone(),
                window_index: None,
                pane_index: None,
            }),
            state: crate::popup::PopupStateKind::Previous,
            source: crate::popup::PopupRowSource::Registry,
            title: record.name.clone(),
            summary: format!("{} {}", record.state, record.path.display()),
            last_changed: Some(record.last_seen),
            attachable: false,
        });
    }
    crate::popup::PopupModel::new(rows)
}
```

- [ ] **Step 3: Add cross-session enter result**

Extend `PopupEnterResult`:

```rust
enum PopupEnterResult {
    Reconnect,
    SwitchSession { session: String },
    Message(String),
}
```

Add helper:

```rust
fn perform_popup_enter(
    current_session: &str,
    row: &crate::popup::PopupRow,
) -> PopupEnterResult {
    let Some(target) = row.target.as_ref() else {
        return PopupEnterResult::Message("row is not attachable".to_string());
    };
    if !row.attachable {
        return PopupEnterResult::Message("row is not currently attachable".to_string());
    }
    if target.session == current_session {
        PopupEnterResult::Reconnect
    } else {
        PopupEnterResult::SwitchSession {
            session: target.session.clone(),
        }
    }
}
```

Same-session target application remains in `perform_popup_enter_same_session`.

- [ ] **Step 4: Implement raw attach switch result**

Extend `RawAttachExit`:

```rust
SwitchSession {
    session: String,
    pending_input: Vec<u8>,
},
```

In `run_raw_attach_session`, when `forward_stdin_until_detach` returns
`SwitchSession`, call the existing attach setup path with the new session name.
Keep the same socket and pending input.

- [ ] **Step 5: Implement composed attach switch result**

Extend `LiveSnapshotInputEvent` with:

```rust
SwitchSession(String)
```

When popup enter returns `SwitchSession`, send this event. In
`run_live_snapshot_attach`, handle it by breaking out with a new attach loop
result that the caller can use to attach to the selected session. If the current
function cannot return a switch value cleanly, introduce:

```rust
enum LiveAttachExit {
    Detach,
    SwitchSession { session: String, pending_input: Vec<u8> },
}
```

Then update the caller to loop into the selected session.

- [ ] **Step 6: Add cross-session integration test**

Add to `tests/phase1_cli.rs`:

```rust
#[test]
fn workspace_popup_enter_switches_to_detached_live_session() {
    let socket = unique_socket("workspace-popup-switch");
    let session_a = format!("workspace-popup-a-{}", std::process::id());
    let session_b = format!("workspace-popup-b-{}", std::process::id());

    assert_success(&dmux(&socket, &["new", "-d", "-s", &session_a, "--", "sh", "-lc", "cat"]));
    assert_success(&dmux(&socket, &["new", "-d", "-s", &session_b, "--", "sh", "-lc", "cat"]));

    let mut child = spawn_attached_to_session(&socket, &session_a, &[]);
    child.stdin_mut("workspace popup stdin").write_all(b"\x02A").unwrap();
    child.wait_for_stdout_contains_all(&[&session_b], "workspace popup session b");
    child.stdin_mut("workspace popup stdin").write_all(b"j\rfrom-b\n").unwrap();

    let captured_b = poll_capture(&socket, &session_b, "from-b");
    assert!(captured_b.contains("from-b"), "{captured_b:?}");

    child.stdin_mut("workspace popup stdin").write_all(b"\x02d").unwrap();
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session_a]));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session_b]));
}
```

- [ ] **Step 7: Run focused tests**

```bash
cargo test workspace_popup_model_marks_previous_records_disabled -- --test-threads=1
cargo test workspace_popup_enter_switches_to_detached_live_session -- --test-threads=1
```

Expected: tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/client.rs src/popup.rs tests/phase1_cli.rs
git commit -m "feat: switch sessions from workspace popup"
```

---

### Task 6: Add Attention Ordering And Grouping Toggle

**Files:**
- Modify: `src/popup.rs`
- Modify: `src/client.rs`
- Test: `src/popup.rs`, `tests/phase1_cli.rs`

- [ ] **Step 1: Add grouping tests**

Add to `src/popup.rs` tests:

```rust
#[test]
fn attention_grouping_orders_rows_by_state_priority() {
    let mut rows = vec![
        row("working", "work", PopupRowKind::Item),
        row("needs", "needs", PopupRowKind::Item),
    ];
    rows[0].state = PopupStateKind::Working;
    rows[1].state = PopupStateKind::NeedsInput;

    let grouped = group_rows(rows, PopupGrouping::Attention);

    assert_eq!(grouped.rows[0].title, "Needs input");
    assert_eq!(grouped.rows[1].id, "needs");
    assert_eq!(grouped.rows[2].title, "Working");
    assert_eq!(grouped.rows[3].id, "working");
}
```

Expected red result: `group_rows` does not exist.

- [ ] **Step 2: Implement grouping**

Add to `src/popup.rs`:

```rust
pub fn group_rows(rows: Vec<PopupRow>, grouping: PopupGrouping) -> PopupModel {
    match grouping {
        PopupGrouping::Attention => group_rows_by_attention(rows),
        PopupGrouping::Repo => group_rows_by_repo(rows),
    }
}

fn group_rows_by_attention(rows: Vec<PopupRow>) -> PopupModel {
    let groups = [
        (PopupStateKind::NeedsInput, "Needs input"),
        (PopupStateKind::Alert, "Alerts"),
        (PopupStateKind::Ready, "Ready"),
        (PopupStateKind::Failed, "Failed"),
        (PopupStateKind::Working, "Working"),
        (PopupStateKind::Completed, "Completed"),
        (PopupStateKind::Idle, "Idle"),
        (PopupStateKind::Detached, "Detached"),
        (PopupStateKind::Previous, "Previous"),
        (PopupStateKind::Stale, "Stale"),
    ];
    let mut out = Vec::new();
    for (state, title) in groups {
        let mut group_rows = rows
            .iter()
            .filter(|row| row.kind != PopupRowKind::Header && row.state == state)
            .cloned()
            .collect::<Vec<_>>();
        if group_rows.is_empty() {
            continue;
        }
        out.push(PopupRow {
            id: format!("group:{state:?}"),
            kind: PopupRowKind::Header,
            repo_path: None,
            target: None,
            state,
            source: PopupRowSource::Mux,
            title: title.to_string(),
            summary: String::new(),
            last_changed: None,
            attachable: false,
        });
        out.append(&mut group_rows);
    }
    PopupModel::new(out)
}

fn group_rows_by_repo(rows: Vec<PopupRow>) -> PopupModel {
    let mut repos = rows
        .iter()
        .filter(|row| row.kind != PopupRowKind::Header)
        .map(|row| {
            row.repo_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "Unregistered".to_string())
        })
        .collect::<Vec<_>>();
    repos.sort();
    repos.dedup();

    let mut out = Vec::new();
    for repo in repos {
        out.push(PopupRow {
            id: format!("repo:{repo}"),
            kind: PopupRowKind::Header,
            repo_path: None,
            target: None,
            state: PopupStateKind::Idle,
            source: PopupRowSource::Registry,
            title: repo.clone(),
            summary: String::new(),
            last_changed: None,
            attachable: false,
        });
        out.extend(rows.iter().filter(|row| {
            row.kind != PopupRowKind::Header
                && row
                    .repo_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Unregistered".to_string())
                    == repo
        }).cloned());
    }
    PopupModel::new(out)
}
```

- [ ] **Step 3: Apply grouping when rendering active popup**

Where active popup content is rebuilt, build flat item rows first, then call:

```rust
let grouped = crate::popup::group_rows(rows, state.grouping);
state.ensure_selection(&grouped);
```

Do this for Tree, Workspace, and Attention modes.

- [ ] **Step 4: Wire `Tab` action**

When `PopupInputAction::ToggleGrouping` is received:

```rust
if let Some(state) = popup_state.as_mut() {
    state.toggle_grouping();
}
```

Then redraw without forwarding input to the child PTY.

- [ ] **Step 5: Add grouping integration test**

Add to `tests/phase1_cli.rs`:

```rust
#[test]
fn workspace_popup_tab_switches_to_repo_grouping() {
    let socket = unique_socket("workspace-popup-grouping");
    let registry_path = unique_temp_file("workspace-popup-grouping-registry");
    let registry_env = registry_path.to_string_lossy().to_string();
    let workspace_path = std::env::current_dir().unwrap().display().to_string();
    assert_success(&dmux_with_env(
        &socket,
        &["workspace-add", &workspace_path],
        &[("DEVMUX_WORKSPACE_REGISTRY", registry_env.as_str())],
    ));
    let session = format!("workspace-popup-grouping-{}", std::process::id());
    assert_success(&dmux_with_env(
        &socket,
        &["new", "-d", "-s", &session, "--", "sh", "-lc", "sleep 30"],
        &[("DEVMUX_WORKSPACE_REGISTRY", registry_env.as_str())],
    ));

    let mut child = spawn_pty_attached_dmux_with_env(
        &socket,
        &["attach", "-t", &session],
        100,
        30,
        &[],
        &[("DEVMUX_WORKSPACE_REGISTRY", registry_env.as_str())],
    );
    child.write_all(b"\x02A\t");
    child.wait_for_stdout_contains_all(&[&workspace_path], "repo grouped workspace popup");

    child.write_all(b"\x02d");
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
    let _ = std::fs::remove_file(registry_path);
}
```

- [ ] **Step 6: Run focused tests**

```bash
cargo test attention_grouping_orders_rows_by_state_priority -- --test-threads=1
cargo test workspace_popup_tab_switches_to_repo_grouping -- --test-threads=1
```

Expected: tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/popup.rs src/client.rs tests/phase1_cli.rs
git commit -m "feat: group interactive popup rows"
```

---

### Task 7: Add Read-Only Peek

**Files:**
- Modify: `src/popup.rs`
- Modify: `src/client.rs`
- Test: `src/popup.rs`, `tests/phase1_cli.rs`

- [ ] **Step 1: Add peek bounds test**

Add to `src/popup.rs` tests:

```rust
#[test]
fn peek_text_is_bounded() {
    let long_capture = (0..100)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    let peek = render_peek_text(
        "repo=/tmp/repo\nsession=dev\npane=1",
        &long_capture,
        40,
        8 * 1024,
    );

    assert!(peek.contains("repo=/tmp/repo"));
    assert!(peek.contains("line 99"));
    assert!(peek.lines().count() <= 43);
}
```

Expected red result: `render_peek_text` does not exist.

- [ ] **Step 2: Implement peek text bounds**

Add to `src/popup.rs`:

```rust
pub fn render_peek_text(metadata: &str, capture: &str, max_rows: usize, max_bytes: usize) -> String {
    let mut capture_bytes = capture.as_bytes();
    if capture_bytes.len() > max_bytes {
        capture_bytes = &capture_bytes[capture_bytes.len() - max_bytes..];
    }
    let capture_text = String::from_utf8_lossy(capture_bytes);
    let mut capture_lines = capture_text.lines().map(str::to_string).collect::<Vec<_>>();
    if capture_lines.len() > max_rows {
        capture_lines = capture_lines[capture_lines.len() - max_rows..].to_vec();
    }

    let mut lines = metadata.lines().map(str::to_string).collect::<Vec<_>>();
    if !capture_lines.is_empty() {
        lines.push("capture tail".to_string());
        lines.extend(capture_lines);
    }
    lines.join("\n")
}
```

- [ ] **Step 3: Add client peek builder**

Add near popup model builders in `src/client.rs`:

```rust
fn popup_peek_text(socket: &Path, row: &crate::popup::PopupRow) -> io::Result<String> {
    let Some(target) = row.target.as_ref() else {
        return Ok("row has no target".to_string());
    };
    let mut metadata = vec![
        format!("session={}", target.session),
        format!("state={:?}", row.state),
        format!("title={}", row.title),
        format!("summary={}", row.summary),
    ];
    if let Some(path) = &row.repo_path {
        metadata.push(format!("repo={}", path.display()));
    }
    if let Some(window) = target.window_index {
        metadata.push(format!("window={window}"));
    }
    if let Some(pane) = target.pane_index {
        metadata.push(format!("pane={pane}"));
    }

    let capture = if target.pane_index.is_some() {
        let capture_target = protocol::Target {
            session: target.session.clone(),
            window: target
                .window_index
                .map_or(protocol::WindowTarget::Active, protocol::WindowTarget::Index),
            pane: target
                .pane_index
                .map_or(protocol::PaneTarget::Active, protocol::PaneTarget::Index),
        };
        let body = send_control_request(
            socket,
            &protocol::encode_capture_target(
                &capture_target,
                protocol::CaptureMode::Screen,
                protocol::BufferSelection::All,
            ),
        )?;
        String::from_utf8_lossy(&body).to_string()
    } else {
        String::new()
    };

    Ok(crate::popup::render_peek_text(
        &metadata.join("\n"),
        &capture,
        40,
        8 * 1024,
    ))
}
```

- [ ] **Step 4: Wire `Space` to peek**

When `PopupInputAction::TogglePeek` is received:

```rust
if let Some(state) = popup_state.as_mut() {
    state.peek = !state.peek;
}
```

When rendering the popup, if `state.peek` is true and a selected row exists, call
`popup_peek_text(socket, row)` and pass it to `render_popup_text`.

- [ ] **Step 5: Add integration test**

Add to `tests/phase1_cli.rs`:

```rust
#[test]
fn attention_popup_space_opens_read_only_peek() {
    let socket = unique_socket("attention-popup-peek");
    let session = format!("attention-popup-peek-{}", std::process::id());
    assert_success(&dmux(&socket, &["new", "-d", "-s", &session, "--", "sh", "-lc", "printf peek-ready; sleep 30"]));
    let ready = poll_capture(&socket, &session, "peek-ready");
    assert!(ready.contains("peek-ready"), "{ready:?}");

    assert_success(&dmux(
        &socket,
        &[
            "agent-event",
            "-t",
            &format!("{}:0.0", session),
            "--state",
            "needs_input",
            "--label",
            "choose option",
        ],
    ));

    let mut child = spawn_attached_to_session(&socket, &session, &[]);
    child.stdin_mut("attention peek stdin").write_all(b"\x02! ").unwrap();
    child.wait_for_stdout_contains_all(&["Peek"], "attention peek title");
    child.wait_for_stdout_contains_all(&["choose option"], "attention peek label");
    child.wait_for_stdout_contains_all(&["peek-ready"], "attention peek capture");

    child.stdin_mut("attention peek stdin").write_all(b"\x02d").unwrap();
    assert_success(&wait_for_child_exit(child));
    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
}
```

- [ ] **Step 6: Run focused tests**

```bash
cargo test peek_text_is_bounded -- --test-threads=1
cargo test attention_popup_space_opens_read_only_peek -- --test-threads=1
```

Expected: tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/popup.rs src/client.rs tests/phase1_cli.rs
git commit -m "feat: add read-only popup peek"
```

---

### Task 8: Harden Registry Rows For Previous And Stale Sessions

**Files:**
- Modify: `src/registry.rs`
- Modify: `src/client.rs`
- Test: `src/registry.rs`, `tests/phase1_cli.rs`

- [ ] **Step 1: Add registry metadata test**

In `src/registry.rs`, add a test:

```rust
#[test]
fn registry_round_trips_optional_session_metadata() {
    let registry = WorkspaceRegistry {
        workspaces: vec![WorkspaceRecord {
            path: PathBuf::from("/tmp/repo"),
        }],
        sessions: vec![SessionRecord {
            name: "old".to_string(),
            path: PathBuf::from("/tmp/repo"),
            state: "stopped".to_string(),
            last_seen: 42,
            last_window: Some(1),
            last_pane: Some(2),
        }],
    };

    let rendered = render(&registry);
    let parsed = parse(&rendered).unwrap();

    assert_eq!(parsed, registry);
}
```

Expected red result: `last_window` and `last_pane` do not exist.

- [ ] **Step 2: Extend `SessionRecord`**

Update `SessionRecord` in `src/registry.rs`:

```rust
pub struct SessionRecord {
    pub name: String,
    pub path: PathBuf,
    pub state: String,
    pub last_seen: u64,
    pub last_window: Option<usize>,
    pub last_pane: Option<usize>,
}
```

- [ ] **Step 3: Keep v1 parser backward-compatible**

Update `parse` so both old and new session records are accepted:

```rust
["session", name, path, state, last_seen] => {
    push_session_record(&mut registry, line_index, name, path, state, last_seen, None, None)?;
}
["session", name, path, state, last_seen, last_window, last_pane] => {
    push_session_record(
        &mut registry,
        line_index,
        name,
        path,
        state,
        last_seen,
        parse_optional_usize(last_window, line_index, "last_window")?,
        parse_optional_usize(last_pane, line_index, "last_pane")?,
    )?;
}
```

Add helpers:

```rust
fn parse_optional_usize(
    value: &str,
    line_index: usize,
    field: &str,
) -> io::Result<Option<usize>> {
    if value.is_empty() {
        return Ok(None);
    }
    value.parse::<usize>().map(Some).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("line {}: invalid {field}", line_index + 1),
        )
    })
}

fn push_session_record(
    registry: &mut WorkspaceRegistry,
    line_index: usize,
    name: &str,
    path: &str,
    state: &str,
    last_seen: &str,
    last_window: Option<usize>,
    last_pane: Option<usize>,
) -> io::Result<()> {
    let last_seen = last_seen.parse::<u64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("line {}: invalid last_seen", line_index + 1),
        )
    })?;
    registry.sessions.push(SessionRecord {
        name: decode_text(name).map_err(|message| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {}: {message}", line_index + 1),
            )
        })?,
        path: decode_path(path).map_err(|message| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {}: {message}", line_index + 1),
            )
        })?,
        state: state.to_string(),
        last_seen,
        last_window,
        last_pane,
    });
    Ok(())
}
```

- [ ] **Step 4: Render metadata fields**

Update `render` session line:

```rust
lines.push(format!(
    "session\t{}\t{}\t{}\t{}\t{}\t{}",
    encode_text(&session.name),
    encode_path(&session.path),
    session.state,
    session.last_seen,
    session.last_window.map(|value| value.to_string()).unwrap_or_default(),
    session.last_pane.map(|value| value.to_string()).unwrap_or_default()
));
```

Update every `SessionRecord` construction in `src/registry.rs`, `src/main.rs`,
and tests to include `last_window: None, last_pane: None`.

- [ ] **Step 5: Use previous target metadata in workspace rows**

In `workspace_popup_model`, build previous targets with:

```rust
target: Some(crate::popup::PopupTarget {
    session: record.name.clone(),
    window_index: record.last_window,
    pane_index: record.last_pane,
}),
```

Keep `kind: DisabledItem` and `attachable: false`.

- [ ] **Step 6: Run focused tests**

```bash
cargo test registry_round_trips_optional_session_metadata -- --test-threads=1
cargo test workspace_popup_model_marks_previous_records_disabled -- --test-threads=1
```

Expected: tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/registry.rs src/main.rs src/client.rs tests/phase1_cli.rs
git commit -m "feat: preserve previous session popup metadata"
```

---

### Task 9: Normalize Agent Event Schema

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/protocol.rs`
- Modify: `src/server.rs`
- Modify: `src/client.rs`
- Test: `src/cli.rs`, `src/protocol.rs`, `tests/phase1_cli.rs`

- [ ] **Step 1: Add CLI parse test for optional source and timestamp**

In `src/cli.rs` tests:

```rust
#[test]
fn parses_agent_event_source_and_timestamp() {
    assert_eq!(
        parse_args([
            "dmux",
            "agent-event",
            "-t",
            "dev:0.1",
            "--state",
            "needs_input",
            "--label",
            "permission requested",
            "--source",
            "codex",
            "--changed-at",
            "123",
        ])
        .unwrap(),
        Command::AgentEvent {
            target: protocol::Target {
                session: "dev".to_string(),
                window: protocol::WindowTarget::Index(0),
                pane: protocol::PaneTarget::Index(1),
            },
            state: "needs_input".to_string(),
            label: "permission requested".to_string(),
            source: Some("codex".to_string()),
            changed_at: Some(123),
        }
    );
}
```

Expected red result: fields do not exist.

- [ ] **Step 2: Extend CLI command**

Update `Command::AgentEvent`:

```rust
AgentEvent {
    target: protocol::Target,
    state: String,
    label: String,
    source: Option<String>,
    changed_at: Option<u64>,
},
```

Update `parse_agent_event` to support:

```text
--source <text>
--changed-at <unix-seconds>
```

Keep current command syntax valid by defaulting both to `None`.

- [ ] **Step 3: Extend protocol request**

Update `protocol::Request::AgentEvent` with `source` and `changed_at`.

Add protocol encoding:

```rust
pub fn encode_agent_event(
    target: &Target,
    state: &str,
    label: &str,
    source: Option<&str>,
    changed_at: Option<u64>,
) -> String {
    format!(
        "AGENT_EVENT\t{}\t{}\t{}\t{}\t{}\n",
        encode_target(target),
        encode_hex(state.as_bytes()),
        encode_hex(label.as_bytes()),
        encode_hex(source.unwrap_or("").as_bytes()),
        changed_at.map(|value| value.to_string()).unwrap_or_default()
    )
}
```

Update decoder to accept both the old 4-field request and the new 6-field
request so older clients still work.

- [ ] **Step 4: Store source and changed_at on panes**

Update `PaneAgentEvent` in `src/server.rs`:

```rust
struct PaneAgentEvent {
    state: String,
    label: String,
    source: String,
    changed_at: Option<u64>,
}
```

Add pane format fields:

```rust
#{pane.agent_source}
#{pane.agent_changed_at}
```

Update `expand_pane_format` to replace those tokens.

- [ ] **Step 5: Add client list format fields**

Update `ATTENTION_LIST_PANES_FORMAT` and `TREE_LIST_PANES_FORMAT` to include
agent source and changed timestamp after `#{pane.agent_label}`:

```rust
#{pane.agent_source}\u{1f}#{pane.agent_changed_at}
```

Update parsers and row builders accordingly.

- [ ] **Step 6: Add integration test**

Add to `tests/phase1_cli.rs`:

```rust
#[test]
fn agent_event_source_and_changed_at_feed_attention_popup() {
    let socket = unique_socket("agent-event-schema");
    let session = format!("agent-event-schema-{}", std::process::id());
    assert_success(&dmux(&socket, &["new", "-d", "-s", &session, "--", "sh", "-lc", "sleep 30"]));

    assert_success(&dmux(
        &socket,
        &[
            "agent-event",
            "-t",
            &format!("{}:0.0", session),
            "--state",
            "ready",
            "--label",
            "review ready",
            "--source",
            "codex",
            "--changed-at",
            "123",
        ],
    ));

    let output = dmux(
        &socket,
        &[
            "list-panes",
            "-t",
            &session,
            "-F",
            "#{pane.agent_state}\t#{pane.agent_label}\t#{pane.agent_source}\t#{pane.agent_changed_at}",
        ],
    );
    assert_success(&output);
    let listing = String::from_utf8_lossy(&output.stdout);
    assert!(listing.contains("ready\treview ready\tcodex\t123"), "{listing:?}");

    assert_success(&dmux(&socket, &["kill-session", "-t", &session]));
}
```

- [ ] **Step 7: Run focused tests**

```bash
cargo test parses_agent_event_source_and_timestamp -- --test-threads=1
cargo test round_trips_agent_event_request -- --test-threads=1
cargo test agent_event_source_and_changed_at_feed_attention_popup -- --test-threads=1
```

Expected: tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/cli.rs src/protocol.rs src/server.rs src/client.rs tests/phase1_cli.rs
git commit -m "feat: normalize agent event metadata"
```

---

### Task 10: Update Help And README

**Files:**
- Modify: `src/cli.rs`
- Modify: `README.md`
- Test: `src/cli.rs`

- [ ] **Step 1: Update attach help text**

In `src/cli.rs`, update `attach_help` and `attach_help_overlay` popup lines to
make the interactive behavior explicit:

```text
C-b ! attention navigator   C-b w tree navigator   C-b A workspaces
popup: j/k move  Enter focus/attach  Space peek  Tab group  / filter  Esc close
```

Keep `C-b i detail popup` as a read-only detail popup until it is moved into the
interactive model.

- [ ] **Step 2: Update README current capabilities**

In `README.md`, update the attach popup capability bullet to mention:

```markdown
- interactive attach popups for attention, current-session tree, and workspace
  navigation; `Enter` focuses or attaches, `Space` opens read-only peek, `Tab`
  toggles grouping, and `/` filters rows
```

Also add a short limit note:

```markdown
- popup reply, dispatch, pin/rename, stop/delete, PR status, and worktree
  management are intentionally future slices
```

- [ ] **Step 3: Run help-focused tests**

Run:

```bash
cargo test attach_help -- --test-threads=1
cargo test list_keys -- --test-threads=1
```

Expected: tests pass. If tests assert old help text, update them to the new
navigator wording.

- [ ] **Step 4: Commit**

```bash
git add src/cli.rs README.md tests/phase1_cli.rs
git commit -m "docs: document interactive popup navigation"
```

---

### Task 11: Full Verification

**Files:**
- No source edits expected unless verification reveals a bug.

- [ ] **Step 1: Format check**

```bash
cargo fmt -- --check
```

Expected: exits 0.

- [ ] **Step 2: Focused popup tests**

```bash
cargo test popup -- --test-threads=1
cargo test workspace -- --test-threads=1
cargo test attention -- --test-threads=1
```

Expected: exits 0.

- [ ] **Step 3: Full test suite**

```bash
cargo test -- --test-threads=1
```

Expected: exits 0.

- [ ] **Step 4: Whitespace check**

```bash
git diff --check
```

Expected: exits 0.

- [ ] **Step 5: Manual smoke**

Run a quick local attach smoke:

```bash
target/debug/dmux kill-server || true
target/debug/dmux new -d -s popup-smoke -- sh -lc 'printf ready; sleep 60'
target/debug/dmux split-window -t popup-smoke -h -- sh -lc 'cat'
target/debug/dmux attach -t popup-smoke
```

Inside attach:

```text
C-b w
j
Enter
typed-from-popup
C-b !
Space
Esc
C-b d
```

Expected:

- `C-b w` opens an interactive tree popup.
- `j` moves selection and is not typed into the pane.
- `Enter` focuses the selected pane.
- `typed-from-popup` reaches the focused pane.
- `C-b !` opens the attention navigator.
- `Space` opens a read-only peek.
- `Esc` closes peek before closing the popup.
- `C-b d` detaches cleanly.

Clean up:

```bash
target/debug/dmux kill-session -t popup-smoke || true
```

- [ ] **Step 6: Commit any verification fix**

If verification required source changes, stage the exact files changed by the
fix and commit them:

```bash
git status --short
git add src/client.rs src/popup.rs tests/phase1_cli.rs
git commit -m "fix: stabilize interactive popup navigation"
```

If the verification fix touched a different tracked file, replace the `git add`
path list with the concrete paths shown by `git status --short`. If no source
fixes were required, do not create an empty commit.

---

### Task 12: Follow-Up Plan Boundaries

**Files:**
- No file changes.

- [ ] **Step 1: Confirm this plan's feature boundary**

After Task 11 passes, verify these behaviors exist:

- rows are selectable
- `Enter` focuses or attaches
- `Space` opens read-only peek
- `Tab` changes grouping
- `/` filters
- previous/stale rows are visible but disabled
- explicit agent events drive attention states

- [ ] **Step 2: Do not add management features in this branch**

Do not add:

- reply from peek
- dispatch new agents
- pin or reorder
- rename
- stop/delete
- PR/check dots
- worktree lifecycle
- desktop notifications

Those features require separate specs and plans because they introduce
agent-specific input routing, destructive actions, or background process
management.

- [ ] **Step 3: Prepare next recommendation**

When this plan is complete, recommend the next plan in this order:

1. Reply from peek for explicit `needs_input` rows.
2. Pin/rename/reorder as non-destructive list management.
3. Stop/delete with confirmation.
4. Dispatch/background session creation.
5. Worktree lifecycle and PR/check status.

No commit is required for this task unless a new follow-up spec or plan is
created.
