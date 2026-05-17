# devmux

`dmux` is an early Rust terminal multiplexer prototype.

Implemented Phase 0/1 commands:

- `dmux new -d -s <name> [-- command...]`
- `dmux new -s <name> [-- command...]`
- `dmux`
- `dmux attach [-t <name>]`
- `dmux ls [-F <format>]`
- `dmux list-sessions [-F <format>]`
- `dmux rename-session -t <old-name> <new-name>`
- `dmux list-clients [-t <name>] [-F <format>]`
- `dmux detach-client [-t <name>] [-c <client-id>]`
- `dmux capture-pane -t <name> -p [--screen|--history|--all] [--start-line <n> --end-line <n>|--search <text> [--match <n>]]`
- `dmux copy-mode -t <name> [--screen|--history|--all] [--search <text> [--match <n>]]`
- `dmux save-buffer -t <name> [-b <buffer>] [--screen|--history|--all] [--start-line <n> --end-line <n>|--search <text> [--match <n>]]`
- `dmux list-buffers [-F|--format <format>]`
- `dmux paste-buffer -t <name> [-b <buffer>]`
- `dmux delete-buffer -b <buffer>`
- `dmux resize-pane -t <name> -x <cols> -y <rows>`
- `dmux resize-pane -t <name> -L|-R|-U|-D [amount]`
- `dmux send-keys -t <name> <keys...>`
- `dmux new-window -t <name> [-- command...]`
- `dmux list-windows -t <name> [-F <format>]`
- `dmux select-window -t <name> -w <index>|--window-id <id>|-n <name>`
- `dmux rename-window -t <name> [-w <index>|--window-id <id>|-n <old-name>] <new-name>`
- `dmux next-window -t <name>`
- `dmux previous-window -t <name>`
- `dmux kill-window -t <name> [-w <index>]`
- `dmux new-tab -t <name> [-- command...]`
- `dmux list-tabs -t <name> [-F <format>]`
- `dmux select-tab -t <name> -i <index>|--tab-id <id>|-n <name>`
- `dmux rename-tab -t <name> [-i <index>|--tab-id <id>|-n <old-name>] <new-name>`
- `dmux next-tab -t <name>`
- `dmux previous-tab -t <name>`
- `dmux kill-tab -t <name> [-i <index>]`
- `dmux split-window -t <name> -h|-v [-- command...]`
- `dmux split -t <name> -h|-v [-- command...]`
- `dmux list-panes -t <name> [-F <format>]`
- `dmux select-pane -t <name> -p <index>|--pane-id <id>|-L|-R|-U|-D`
- `dmux kill-pane -t <name> [-p <index>]`
- `dmux respawn-pane -t <name> [-p <index>] [-k] [-- command...]`
- `dmux zoom-pane -t <name> [-p <index>]`
- `dmux status-line -t <name> [-F <format>]`
- `dmux display-message -t <name> -p <format>`
- `dmux kill-session -t <name>`
- `dmux kill-server`

Commands that operate on panes accept structured targets as
`<session>[:<window>[.<pane>]]`. Numeric window and pane values are indexes,
`@<id>` selects a window ID, `%<id>` selects a pane ID, and `=<name>` selects a
window name. Session names cannot contain `:` because it separates structured
targets.

`capture-pane -p` defaults to combined scrollback history plus the current
screen. Use `--screen` for only the current screen, `--history` for only
scrollback, or `--all` for the combined output explicitly.

`copy-mode` currently prints command-driven line inspection output as
`<line-number><tab><text>` from the same `--screen`, `--history`, and `--all`
capture sources as `capture-pane`; default and `--all` are combined history plus
screen. Use `--search` to show only matching lines while preserving their
original line numbers, and `--match <n>` to show one indexed match. Missing
matches are reported as errors.

Running `dmux` with no command opens the `default` session, creating it first
when needed. `dmux attach` without `-t` also targets `default`. Explicit
`dmux ls`, `dmux attach -t <name>`, and `dmux kill-session -t <name>` report
when no server is running instead of starting an empty daemon.

Attached clients can enter the current basic copy-mode view with `C-b [`.
Inside copy-mode, `j`/`k`, arrows, `Ctrl-n`/`Ctrl-p`, PageUp/PageDown, and
Home/End-style `g`/`G` move the cursor and viewport, `y` or Enter saves the
current line to a buffer, and `q` or Escape exits. Mouse click saves one
rendered line; mouse drag saves an inclusive line range. Mouse selection is
currently basic line-level selection. In unzoomed multi-pane attach, copy-mode
copies lines from the rendered composed layout, including pane separators and
visible content from multiple panes, while input is routed to the server active
pane. Unzoomed multi-pane attach handles `C-b d` to detach (`C-b D` also detaches), `C-b ?` to show
attach help, `C-b :` to run basic attached commands such as rename/select/kill/list/paste, `C-b c` to create a new window, `C-b n`/`C-b p` to cycle windows,
`C-b %` to split right, `C-b "` to split down, `C-b h/j/k/l` to focus by
direction, `C-b H/J/K/L` to resize the active pane left/down/up/right by
5 cells, `C-b o` to cycle the server active pane, `C-b q` followed by a single
digit to select a pane by number, `C-b x` to close the active pane, `C-b z` to
toggle zoom, and mouse click to select a pane. `C-b c`, `C-b n`, `C-b p`,
`C-b %`, and `C-b "` also work
from a fresh single-pane attach and transition automatically into the multi-pane
layout view. Pane splitting is also available with
`dmux split-window -t <name> -h|-v [-- command...]`; active pane focus is
available with `dmux select-pane -t <name> -p <index>|--pane-id <id>|-L|-R|-U|-D`;
active pane resizing is available with
`dmux resize-pane -t <name> -L|-R|-U|-D [amount]`; use `dmux attach --help` or
`dmux help attach` to list attach-time bindings.
Unzoomed multi-pane attach redraws from server change events and keeps a polling
fallback for mixed-version daemons or missed events.

`save-buffer` stores captured active-pane text in an in-memory buffer with a
1 MiB per-buffer limit and a 50-buffer server limit. Use `-b` to name the
buffer, omit `-b` to create an automatic name, and omit `-b` on `paste-buffer`
to paste the latest saved buffer. Re-saving a named buffer replaces it and makes
it latest. `capture-pane` and `save-buffer` support 1-based inclusive
`--start-line`/`--end-line` ranges; negative values count back from the captured
tail, so `--start-line -2 --end-line -1` selects the last two lines.
`--search` selects matching lines, and `--match <n>` selects a specific match.
`list-buffers` defaults to `<name><tab><bytes><tab><preview>` and supports
format fields `#{buffer.index}`, `#{buffer.name}`, `#{buffer.bytes}`,
`#{buffer.lines}`, `#{buffer.latest}`, and `#{buffer.preview}`. Missing buffers,
missing matches, and pastes into exited panes are explicit errors.

`status-line -F` and `display-message -p` expand session/window/tab/pane/client
fields plus latest-buffer fields: `#{buffer.count}`, `#{buffer.index}`,
`#{buffer.name}`, `#{buffer.bytes}`, `#{buffer.lines}`, `#{buffer.latest}`, and
`#{buffer.preview}`. Unknown `#{...}` tokens are left literal, and expanded data
is not expanded a second time. Attached status includes client and buffer counts;
`display-message` also shows a bounded transient message in attached layout UIs.

Implemented Phase 2 groundwork:

- basic terminal screen state for printable shell output
- scrollback-backed `capture-pane -p`
- explicit screen/history/all capture modes for `capture-pane`
- SGR escape stripping in captures
- carriage-return overwrite handling in captures
- explicit PTY resize requests with screen-state resize
- detached session input through `send-keys`
- attach-time PTY resize to the current terminal size when available
- session formats: `#{session.name}`, `#{session.windows}`, `#{session.window_count}`, `#{session.attached}`, `#{session.attached_count}`, `#{session.created_at}`, and `#{client.count}`
- client formats: `#{client.id}`, `#{client.session}`, `#{client.type}`, `#{client.attached}`, `#{client.width}`, and `#{client.height}`
- session rename plus attach client listing and command-driven detach
- attached clients request PTY resize on terminal `SIGWINCH`
- minimal window tracking with active window selection
- tab command aliases over the window model
- window removal while keeping the session alive
- named windows/tabs with stable IDs, index/ID/name selection, rename, and cycling
- window/tab list formats: `#{window.index}`, `#{window.id}`, `#{window.name}`,
  `#{window.active}`, `#{window.panes}` and matching `#{tab.*}` fields
- split-pane sessions with a server-side active pane
- active pane selection by pane index, stable ID, or layout direction
- pane removal while keeping the session alive
- exited panes remain listed/inspectable until killed or respawned
- pane lifecycle formats: `#{pane.state}`, `#{pane.pid}`,
  `#{pane.exit_status}`, and `#{pane.exit_signal}`
- in-place pane respawn, with `-k` required to replace a running pane
- pane zoom state while keeping all panes alive
- server-side statusline format expansion
- stable pane IDs exposed as `#{pane.id}` in pane/status formats
- stable tab/window IDs plus active name/index/count exposed in status formats
- in-memory buffers backed by pane capture and paste into active panes
- command-driven line range, tail range, and search-match selection for capture
  and buffer saves
- command-driven line-numbered copy-mode inspection with search filtering and
  indexed match selection
- formatted buffer listing with latest marker, byte/line counts, and previews
- latest-buffer status/display-message format fields with token-like data safety
- bounded transient display messages in attach render output
- attach-time basic copy-mode key handling for line copy
- attach-time basic copy-mode mouse selection for line ranges
- attach-time statusline snapshot rendering
- attach-time split-pane layout snapshot rendering
- event-driven live redraw for multi-pane attach with polling fallback
- active-pane input routing for multi-pane attach
- attach-time pane cycling for multi-pane attach
- attach-time numbered pane selection for multi-pane attach
- attach-time mouse focus for multi-pane attach
- multi-pane attach copy-mode over the rendered composed layout
- attach layout pane-region mapping foundation
- `DEVMUX_ATTACH_SIZE=<cols>x<rows>` override for tests and automation

Current limits:

- multi-pane attach routes input to the server active pane
- zoomed panes are tracked server-side and keep single-pane live attach behavior
- attach-time statusline rendering is snapshot-only for raw single-pane attach
- in-memory screen and scrollback only
- copy-mode selection is line-based only
- buffer contents are in-memory only
- full terminal protocol support is incomplete
- no named layout support yet
- Unix/macOS POSIX PTY support only

Supported `send-keys` tokens are literal text plus `Enter`, `Space`, `Tab`,
`Escape`, and `C-c`.
