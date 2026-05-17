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
- `dmux select-layout -t <name> even-horizontal|even-vertical|tiled|main-horizontal|main-vertical`
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
- `dmux swap-pane -s <target> -t <target>`
- `dmux move-pane -s <target> -t <target> [-h|-v]`
- `dmux break-pane -t <target>`
- `dmux join-pane -s <target> -t <target> [-h|-v]`
- `dmux kill-pane -t <name> [-p <index>]`
- `dmux respawn-pane -t <name> [-p <index>] [-k] [-- command...]`
- `dmux zoom-pane -t <name> [-p <index>]`
- `dmux status-line -t <name> [-F <format>]`
- `dmux display-message -t <name> -p <format>`
- `dmux run <command; command...>`
- `dmux command <command; command...>`
- `dmux source-file <path>`
- `dmux run-shell <shell-command>`
- `dmux list-keys [-F <format>]`
- `dmux bind-key <key> <supported-live-action>`
- `dmux unbind-key <key>`
- `dmux show-options [-F <format>]`
- `dmux set-option <name> <value>`
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

Automation commands parse dmux commands directly without shell evaluation.
`dmux run 'new -d -s dev; split-window -t dev -v'` executes commands in order
and stops on the first parse or server error, reporting which command failed.
Use quotes or backslashes for spaces and semicolons inside one argument.
`dmux source-file <path>` reads newline-separated dmux commands; blank lines
and lines whose first non-whitespace character is `#` are ignored, and errors
include the file line. `dmux run-shell <shell-command>` runs one host shell
command synchronously, returns a non-zero status on failure, and prints at most
64 KiB from each output stream.

Attached clients show a top tab line for windows and a bottom info line with
the active session, pane, client/buffer counts, alerts, and quick affordances
such as `C-b ? help`, `C-b : command`, and Alt-arrow focus. Press `C-b ?` to
toggle a boxed attach help popup covering session, window, pane, copy-mode, and
command-prompt workflows; the popup stays visible until closed. Press `C-b !`
to open an on-demand attention popup for mux-level pane alerts such as bell,
activity, exited panes, and blocked clipboard attempts. Press `C-b :`
for an attached command prompt that
shows the typed command and controls (`Enter` run, Escape/`C-c` cancel,
Backspace edit), with examples such as `:split -h`, `:split -v`,
`:layout tiled`, `:swap-pane 1`, `:break-pane`, and `:list-windows`. Unknown attached
commands report a hint to use `C-b ?` and show common examples. The prompt
also accepts semicolon-separated prompt commands and `:source-file <path>` for
newline-separated prompt commands with blank lines and leading-`#` comments
ignored.

Key bindings and options are runtime/server-scoped and reset when the dmux
server exits. Use `dmux list-keys` to inspect bindings and
`dmux bind-key <key> <action>`/`dmux unbind-key <key>` to customize supported
live actions such as `copy-mode`, `detach-client`, `command-prompt`,
`split-window -h|-v`, `select-pane -L|-D|-U|-R`, `resize-pane -L|-D|-U|-R [amount]`,
`next-pane`, `display-panes`, `show-attention`, `new-window`, `next-window`,
`previous-window`, `kill-pane`, and `zoom-pane`. Keys can be printable
characters, `Space`, `Tab`, `Enter`, `Escape`, arrows, `C-<letter>`,
`C-<arrow>`, or `M-<key>`/`Alt-<key>`. Ordinary
bindings are prefix chords; `M-`/`Alt-` bindings are no-prefix attach shortcuts.
Use `dmux show-options` and `dmux set-option prefix C-a` to inspect or change
validated runtime options; `status-hints` accepts `on` or `off`.

Attached clients can enter the current basic copy-mode view with `C-b [`.
Inside copy-mode, `j`/`k`, arrows, `Ctrl-n`/`Ctrl-p`, PageUp/PageDown, and
Home/End-style `g`/`G` move the cursor and viewport, `y` or Enter saves the
current line to a buffer, and `q` or Escape exits. Mouse click saves one
rendered line; mouse drag saves an inclusive line range. Mouse selection is
currently basic line-level selection. In unzoomed multi-pane attach, copy-mode
copies lines from the rendered composed layout, including pane separators and
visible content from multiple panes, while input is routed to the server active
pane. Unzoomed multi-pane attach handles `C-b d` to detach (`C-b D` also
detaches), `C-b ?` to toggle attach help, `C-b !` to toggle the attention
popup, `C-b :` to run attached commands such as
rename/select/kill/list/paste/split/layout, `C-b c` to create a new window,
`C-b n`/`C-b p` to cycle windows,
`C-b %` to split right, `C-b "` to split down, `C-b h/j/k/l` or `C-b` arrows to focus by
direction, `Alt-h/j/k/l` or `Alt` arrows to focus without prefix, `C-b H/J/K/L`
to resize the active pane left/down/up/right by 5 cells, `C-b Ctrl-arrows` to
resize by 1 cell, `C-b o` to cycle the server active pane, `C-b q` followed by
a single digit to select a pane by number, `C-b x` to close the active pane,
`C-b z` to toggle zoom, and mouse click to select a pane. `C-b c`, `C-b n`, `C-b p`,
`C-b %`, and `C-b "` also work
from a fresh single-pane attach and transition automatically into the multi-pane
layout view. Pane splitting is also available with
`dmux split-window -t <name> -h|-v [-- command...]`; active pane focus is
available with `dmux select-pane -t <name> -p <index>|--pane-id <id>|-L|-R|-U|-D`;
active pane resizing is available with
`dmux resize-pane -t <name> -L|-R|-U|-D [amount]`; pane layouts are available
with `dmux select-layout -t <name> tiled|even-horizontal|even-vertical|main-horizontal|main-vertical`
or attach prompt `:layout tiled`. Pane composition is available with
`dmux swap-pane`, `dmux move-pane`, `dmux break-pane`, and `dmux join-pane`;
these preserve pane IDs and running processes while recomputing layouts. Moving
the last pane out of a window removes the empty source window. `main-horizontal`
and `main-vertical` use the
active pane as the main pane; use `dmux attach --help` or
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
fields, including pane metadata fields `#{pane.cwd}`, `#{pane.title}`,
`#{pane.bell}`, `#{pane.activity}`, and `#{pane.clipboard_blocked}`, plus
latest-buffer fields:
`#{buffer.count}`, `#{buffer.index}`,
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
- client cwd propagation for new panes, windows, and respawns
- OSC 7 pane cwd tracking for child-reported directory changes
- OSC 0/2 pane title tracking
- pane bell and activity state exposed through pane/status formats
- OSC 52 clipboard writes blocked by default and counted as
  `#{pane.clipboard_blocked}`
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
- top tab line, bottom info line, persistent attach help/attention popups, and
  visible command-prompt input and controls
- attach-time basic copy-mode key handling for line copy
- attach-time basic copy-mode mouse selection for line ranges
- attach-time TUI chrome with tabs, footer info, and prefix/help/command affordances
- attach-time split-pane layout snapshot rendering
- event-driven live redraw for multi-pane attach with polling fallback
- synchronized-output redraw gating with an end-marker flush and timeout guard
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
- raw single-pane attach still receives one-time attach chrome before raw PTY streaming
- in-memory screen and scrollback only
- copy-mode selection is line-based only
- buffer contents are in-memory only
- full terminal protocol support is incomplete
- no named layout support yet
- Unix/macOS POSIX PTY support only

Supported `send-keys` tokens are literal text plus `Enter`, `Space`, `Tab`,
`Escape`, and `C-c`.
