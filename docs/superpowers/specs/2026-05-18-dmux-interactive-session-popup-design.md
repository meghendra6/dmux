# dmux Interactive Session Popup Design

## Goal

Turn dmux popups from read-only viewers into interactive modal control surfaces.
The default attach screen remains a simple tmux-style pane layout. When the user
opens a popup, dmux temporarily overlays a keyboard-driven navigator for choosing
the next session, pane, or agent-related item that needs attention.

This design is inspired by Claude Code agent view's row-based session management:
sessions are grouped by state, rows can be selected, `Space` peeks at context,
and `Enter` attaches to the selected session. dmux should adapt that loop to a
terminal multiplexer: the popup floats over existing panes, and `Enter` moves the
current terminal to the selected dmux session/window/pane.

Reference: <https://code.claude.com/docs/en/agent-view>

## Core Decisions

- Popup rows are actionable. A popup is not just a status viewer.
- `Enter` immediately focuses, attaches, or switches to the selected row.
- `Space` opens a read-only peek panel in the first implementation.
- The row model is hybrid: rows are pane/task items, while repo/session/window
  groups provide structure.
- The default grouping is attention-first.
- `Tab` toggles between attention-first and repo-first grouping.
- The popup must not duplicate agent statusline details such as model, context,
  token usage, cost, or runtime.
- Reply, dispatch, pin, rename, stop, delete, and destructive cleanup are later
  management features, not part of the first interactive slice.
- Popup rendering and input handling must never write UI bytes into child PTYs.

## Entry Points

Existing popup keys remain, but they enter the same interactive popup framework
with different scopes:

- `C-b !`: open the interactive navigator in attention mode. Rows with
  `needs_input`, `alert`, `failed`, `ready`, or `completed` states are prioritized.
- `C-b A`: open the workspace/session navigator across registered workspaces,
  live sessions, detached live sessions, and previous session records.
- `C-b w`: open the current-session tree navigator with the same movement,
  selection, peek, and focus behavior.

This keeps the current keyboard vocabulary while avoiding three separate popup
implementations.

## Screen Model

The popup overlays the current attach layout. The underlying panes keep running.
While the popup is active, popup navigation keys are consumed by the client and
are not forwarded to the active child PTY.

Example attention-first view:

```text
Needs input
> ! claude   repo/api    s:auth p:2   permission?       12s
  ! codex    repo/ui     s:fix  p:1   review ready      2m

Alerts
  x codex    repo/core   s:main p:0   command failed    4m

Working
  * codex    repo/dmux   s:dmux p:0   running tests     44s

Completed
  + claude   repo/docs   s:docs p:1   PR opened         9m

Enter: focus/attach   Space: peek   Tab: group   /: filter   Esc: close
```

ASCII markers are used in committed docs and tests. The renderer may use color
or symbols at runtime only where the terminal path already supports them.

## Keyboard Behavior

Common popup keys:

- `j`, `k`, `Down`, `Up`: move selection between selectable rows.
- `PageDown`, `PageUp`: scroll by a page.
- `Home`, `End`: jump to first or last selectable row.
- `Enter`, `Right`: focus, attach, or switch to the selected row.
- `Space`: toggle read-only peek for the selected row.
- `Tab`: toggle attention-first and repo-first grouping.
- `/`: enter filter mode.
- `Esc`: close peek, clear filter, or close popup, in that order.
- `q`: close popup when not typing in filter mode.

Filter mode:

- Typed text filters rows by repo path, session name, window name, pane index,
  pane title, cwd, state, and event label.
- `Backspace` edits the filter.
- `Enter` accepts the current filter and returns to row navigation.
- `Esc` clears the filter; if the filter is already empty, it exits filter mode.

Group headers are not selectable. If filtering hides the selected row, selection
moves to the first visible selectable row.

## Row Model

The client renders a list of `PopupRow` values built from mux state and registry
state.

```text
PopupRow
  id: stable row id for this popup render cycle
  kind: header | item | disabled_item
  repo_path: optional path
  session: dmux session name
  window_id/window_index/window_name: optional window target
  pane_id/pane_index: optional pane target
  state: normalized state
  source: mux | registry | agent_event
  title: short display label
  summary: one-line status or event label
  last_changed: optional timestamp
  attachable: true when Enter can move to it
```

Normalized states:

- `needs_input`: an explicit agent event says the pane is waiting for user input.
- `alert`: bell, inactive activity, blocked clipboard, or explicit alert event.
- `ready`: explicit event says work is ready for review or user pickup.
- `failed`: pane exited unsuccessfully or explicit failure event.
- `working`: explicit event says work is active, or pane is running with recent
  activity.
- `completed`: explicit event says work completed, or pane exited successfully.
- `idle`: pane/session is live but has no known attention state.
- `detached`: live session has no attached clients.
- `previous`: registry record exists but no live session currently matches it.
- `stale`: registry path or previous session record cannot be resolved.

State is intentionally normalized at the mux layer. Claude, Codex, Copilot, or
other tools can feed events later, but the popup should not depend on parsing
their terminal UI.

## Grouping

Attention-first grouping is the default:

1. Needs input
2. Alerts
3. Ready
4. Failed
5. Working
6. Completed
7. Idle and detached
8. Previous and stale

Repo-first grouping uses registered workspace paths as top-level groups. Within
each repo, rows are ordered by the same attention priority. Sessions with no
registered repo path are grouped under `Unregistered`.

The first implementation stores grouping only in the active popup state. Persisted
grouping preferences can be added later after the interaction model is stable.

## Data Sources

The first implementation may use only data dmux already owns or explicit data
sent to dmux:

- live sessions from server session listing
- windows from server window listing
- panes from server pane listing
- pane lifecycle fields such as state, exit status, exit signal, title, cwd,
  bell, activity, clipboard blocked count, and active marker
- workspace registry records
- previous session registry records
- explicit `agent-event` state and label
- bounded capture tail for read-only peek

The popup must not scrape Claude, Codex, Copilot, or shell statuslines for model,
token, context, cost, or runtime details.

## Enter Action

`Enter` is the primary action and should be fast.

- Same session, same window: select the target pane.
- Same session, different window: select the target window, then select the pane.
- Different live session: close the current attach stream and re-enter attach for
  the selected session in the same terminal, then select the target window/pane.
- Detached live session: attach to that session in the same terminal.
- Previous or stale session record with no live session: keep the popup open and
  show a bounded message explaining that the row is not currently attachable.

The first implementation does not respawn stopped sessions from `Enter`. Respawn
or reopen behavior belongs to a later management slice.

## Read-Only Peek

`Space` toggles a peek panel for the selected row. The first peek implementation
is read-only and must not send input to an agent or child PTY.

Peek content:

- repo path
- session/window/pane target
- normalized state
- last explicit agent event label, if any
- pane title and cwd
- exit status or signal, if present
- clipboard blocked count, if nonzero
- bounded capture tail from the pane, up to 40 rows and 8 KiB

When peek is open, `j/k` and arrow movement update the selected row and refresh
the peek content. `Esc` closes peek before closing the popup.

## Registry Semantics

The registry remains local and conservative.

- `workspace-add` registers a repo/workspace path.
- Starting a session records the session and workspace path when dmux can infer
  it.
- Killing a session marks the registry session as stopped.
- Live server state always outranks registry state.
- Previous records are shown so the user can remember earlier work, but they are
  disabled until a later respawn/reopen slice defines safe behavior.
- Stale paths are visible, not silently removed.

This keeps previous sessions discoverable without creating surprising destructive
or restart behavior in the first interactive release.

## Architecture

Add a shared popup controller in the client layer. The server remains the source
of mux facts; the client owns modal input, selection, filtering, grouping, and
overlay rendering.

Suggested units:

- `AttachPopupMode`: which popup entry point is active (`Attention`,
  `Workspace`, `Tree`).
- `PopupState`: selected row, scroll offset, grouping, filter text, and peek
  visibility.
- `PopupRow`: rendered row data and target metadata.
- `PopupModel`: rows plus helper indexes for movement and target lookup.
- `PopupAction`: decoded popup key action.
- `build_popup_model(...)`: collects mux/registry data and produces rows.
- `render_popup(...)`: formats rows and peek content into overlay text.
- `perform_popup_enter(...)`: executes focus/attach/switch behavior.

The controller must be used by both attach paths:

- composed/live snapshot attach
- raw single-pane attach with attach chrome

Raw single-pane attach should still intercept popup keys when the popup is active
and should redraw/clear the overlay through the existing popup rendering path.

## Implementation Stages

### Stage 1: Interactive Popup Foundation

Deliverables:

- popup state with selection, scrolling, grouping, filter text, and peek flag
- common key decoder for popup mode
- unit tests for movement, filtering, grouping toggle, and non-selectable headers
- attach-path tests proving popup keys are consumed and not forwarded to child PTYs

Acceptance:

- existing help/attention/workspace popups can be backed by the new state machine
  without changing their visible content yet.

### Stage 2: Current-Session Tree Navigator

Deliverables:

- `C-b w` uses the interactive row model for current session windows and panes
- `Enter` selects window/pane inside the current session
- same-session focus works while the popup closes cleanly

Acceptance:

- an integration test opens a split session, opens `C-b w`, moves to another pane,
  presses `Enter`, and proves subsequent input reaches that pane.

### Stage 3: Workspace and Live Session Navigator

Deliverables:

- `C-b A` shows registered repos, live sessions, detached live sessions, and
  previous records through the unified row model
- `Tab` toggles attention-first and repo-first grouping
- `Enter` switches the current terminal to another live session
- previous/stale rows are visible but disabled

Acceptance:

- integration tests cover registered path display, detached live session attach,
  cross-session switch, disabled previous row messaging, and grouping toggle.

### Stage 4: Attention Rows and Read-Only Peek

Deliverables:

- `C-b !` opens attention-first mode using the same row model
- mux attention fields and explicit `agent-event` state/label feed normalized row
  states
- `Space` opens read-only peek with bounded capture tail and event metadata

Acceptance:

- tests cover bell/activity/clipboard/exited/agent-event rows, peek rendering,
  peek refresh on selection movement, and capture size bounds.

### Stage 5: Registry Hardening

Deliverables:

- registry records include enough metadata for useful previous-session display
- stale path and stopped session states are explicit
- registry parse/render tests cover forward-compatible unknown-free records
- no destructive cleanup is exposed yet

Acceptance:

- stale paths remain visible, previous records do not become attachable by
  accident, and live sessions override registry records with the same name.

### Stage 6: Agent Event Schema

Deliverables:

- replace ad hoc agent states with documented normalized event values
- keep `agent-event` as the stable ingestion boundary
- add optional tool/source label, summary label, and last-changed timestamp
- document how Claude, Codex, Copilot, and shell hooks can send events without
  requiring terminal UI scraping

Acceptance:

- explicit events can drive `needs_input`, `ready`, `completed`, `failed`, and
  `working` rows without relying on screen text.

### Stage 7: Later Management Features

Future features after focus, switch, and peek are stable:

- reply from peek
- dispatch new background sessions
- pin and reorder rows
- rename rows or sessions
- stop/delete with confirmation
- PR/check status dots
- worktree create/open/list/remove
- desktop notification and clipboard relay

These features should be separate plans because they involve agent-specific
input routing, destructive operations, or long-running background process
management.

## Error Handling

- If data collection fails, render a popup message instead of closing the attach
  session.
- If the selected target disappears before `Enter`, keep the popup open and
  refresh rows.
- If cross-session attach fails, return to the previous attach session when
  possible and show a bounded error message.
- If capture tail fails for peek, show metadata and a short capture error line.
- If terminal size is too small, render a compact popup with only selected row,
  group, and footer hints.

## Test Strategy

Unit tests:

- popup movement skips headers
- selection clamps after filtering
- grouping toggle preserves selected target when possible
- row state normalization priority
- peek content bounds
- registry previous/stale row classification

Integration tests:

- `C-b w` select pane and route input
- `C-b A` cross-session switch
- detached session attach from popup
- disabled previous row does not attach or respawn
- `C-b !` attention row selection from `agent-event`
- popup keys do not leak into child PTYs
- raw attach and composed attach both support popup interaction

Full verification:

```bash
cargo fmt -- --check
cargo test popup -- --test-threads=1
cargo test workspace -- --test-threads=1
cargo test attention -- --test-threads=1
cargo test -- --test-threads=1
git diff --check
```

## Non-Goals

- No always-visible dashboard.
- No terminal UI scraping of agent statuslines.
- No first-version reply box.
- No first-version dispatch UI.
- No first-version destructive stop/delete/remove behavior.
- No new TUI dependency is required for this design.
- No cloud or remote supervisor assumption.

## Success Criteria

The feature succeeds when the user can keep working in a normal multi-pane dmux
layout, open a popup only when needed, see actionable sessions/panes grouped by
attention, and press `Enter` to jump directly to the selected work without
leaving the terminal workflow.
