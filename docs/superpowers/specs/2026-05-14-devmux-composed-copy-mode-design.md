# Devmux Composed Layout Copy Mode Design

## Goal

Make `C-b [` in unzoomed multi-pane attach copy from the rendered composed pane
layout instead of only the server active pane.

This addresses the remaining copy-mode gap after session lifecycle reliability,
attach help, pane cycling, numbered pane selection, and mouse focus. The slice is
intentionally limited to line-level selection over the rendered layout; it does
not add event-driven redraw, horizontal text selection, or a full tmux viewport.

## Current State

- Raw single-pane attach and zoomed attach use the normal live attach path.
- Unzoomed multi-pane attach uses the polling snapshot path and already supports
  `C-b d`, `C-b ?`, `C-b o`, `C-b q`, mouse focus, and `C-b [`.
- The existing `C-b [` path pauses polling redraw and runs the local copy-mode
  UI on the input thread, but it requests `COPY_MODE` from the server and saves
  selected line ranges with `SAVE_BUFFER_LINES`. Both operate on the server
  active pane.
- The server already exposes `ATTACH_LAYOUT_SNAPSHOT`, whose body includes the
  exact rendered snapshot bytes plus pane regions from the same render pass.
- The buffer store can save arbitrary text internally, but the control protocol
  only exposes saves from captured pane text.

## Chosen Design

Use the existing composed snapshot as the copy-mode source only for unzoomed
multi-pane attach. Single-pane attach and zoomed attach keep active-pane
copy-mode behavior.

When live snapshot attach receives `C-b [`, the input thread still pauses redraw
and owns stdin. Instead of calling the active-pane copy-mode source, it reads the
latest `ATTACH_LAYOUT_SNAPSHOT` body, converts the snapshot bytes into numbered
visual lines, and renders the existing copy-mode UI. The first rendered layout
row becomes copy-mode line `1`, the second row becomes line `2`, and so on.

Copied text is saved as the rendered layout text for the selected visual line or
inclusive visual line range. The saved text includes pane separators and padding
exactly as rendered in the snapshot body. Statusline rows, attach help rows, and
pane-number message rows are not part of `ATTACH_LAYOUT_SNAPSHOT`, so they are
not copyable in this slice.

## Protocol Addition

Add a narrow internal control request for saving already-selected text:

```text
SAVE_BUFFER_TEXT\t<session>\t<buffer-name-hex-or-empty>\t<text-hex>\n
```

The response matches existing buffer saves:

```text
OK\n
<saved-buffer-name>\n
```

The server checks that the named session exists before saving, so composed
copy-mode errors remain consistent with other session-scoped requests. No pane
lookup is required because the text has already been selected by the client.

Old-daemon behavior: if a client sends `SAVE_BUFFER_TEXT` to a daemon that does
not know the request, `send_control_request` returns the existing
`unknown request line` error. Copy-mode should convert that into a clear
`composed copy-mode requires an updated dmux server` error and return to attach
without crashing. There is no transparent fallback because old daemons cannot
save arbitrary composed text without changing the copied content.

## Client Flow

1. User presses `C-b [` in unzoomed multi-pane attach.
2. The input thread emits `EnterCopyMode` and pauses polling redraw.
3. The input thread reads `ATTACH_LAYOUT_SNAPSHOT`.
4. The snapshot body bytes are normalized into visual lines for
   `CopyModeView`.
5. Existing copy-mode key and mouse handling moves through those visual lines.
6. `y`, Enter, or mouse release saves selected visual line text with
   `SAVE_BUFFER_TEXT`.
7. Copy-mode exits, the input thread requests `RedrawNow`, and attach resumes.

Raw single-pane attach still uses `COPY_MODE` plus `SAVE_BUFFER_LINES` because
that path can include scrollback/history from the active pane, while a composed
layout snapshot is only the currently rendered layout.

## Selection Semantics

- Line numbers are 1-based visual layout rows, not pane capture history lines.
- Empty rendered rows are selectable and save as blank lines.
- Inclusive mouse drag ranges save in top-to-bottom visual order.
- Separator columns and separator rows are normal rendered text and are copied
  when selected.
- Copy-mode remains line-based; it does not support rectangular or partial-line
  selection.

## Tests

- Protocol round-trip for `SAVE_BUFFER_TEXT`, including hex text with tabs and
  newlines.
- Server handler integration proving `SAVE_BUFFER_TEXT` stores arbitrary text
  and respects missing-session errors.
- Unit tests proving `CopyModeView` can be built from plain rendered text and
  can return selected line/range text.
- Unit tests proving active-pane copy-mode still uses `SAVE_BUFFER_LINES` while
  composed copy-mode uses `SAVE_BUFFER_TEXT`.
- Integration test updating the existing multi-pane `C-b [` coverage so `C-b [`
  plus `y` saves a rendered composed layout line containing both pane outputs,
  not only the active pane.
- Existing help, mouse focus, numbered pane selection, lifecycle smoke, and raw
  attach copy-mode tests remain part of full verification.

## Out Of Scope

- Event-driven redraw or compositor invalidation.
- Copying statusline/help/message rows.
- Horizontal, rectangular, word, or character selection.
- Mapping selected composed text back to source panes.
- Clipboard integration.
- Changing command-line `copy-mode` or `save-buffer` user-facing options.

## Acceptance Criteria

- In unzoomed multi-pane attach, `C-b [` opens copy-mode over the rendered
  composed layout.
- Pressing `y` on the first composed row of a horizontal split saves text that
  includes visible content from both panes and the separator.
- Single-pane and zoomed attach keep active-pane copy-mode behavior.
- Polling redraw stays paused while copy-mode is active and resumes after copy
  or exit.
- If the composed text save request fails, attach reports an error instead of
  silently saving active-pane text.
- README no longer lists composed-layout copy-mode as missing.
