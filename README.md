# devmux

`dmux` is an early Rust terminal multiplexer prototype.

Implemented Phase 0/1 commands:

- `dmux new -d -s <name> [-- command...]`
- `dmux new -s <name> [-- command...]`
- `dmux attach -t <name>`
- `dmux ls`
- `dmux capture-pane -t <name> -p [--screen|--history|--all]`
- `dmux copy-mode -t <name> [--screen|--history|--all] [--search <text>]`
- `dmux save-buffer -t <name> [-b <buffer>] [--screen|--history|--all] [--start-line <n> --end-line <n>|--search <text>]`
- `dmux list-buffers`
- `dmux paste-buffer -t <name> [-b <buffer>]`
- `dmux delete-buffer -b <buffer>`
- `dmux resize-pane -t <name> -x <cols> -y <rows>`
- `dmux send-keys -t <name> <keys...>`
- `dmux new-window -t <name> [-- command...]`
- `dmux list-windows -t <name>`
- `dmux select-window -t <name> -w <index>`
- `dmux kill-window -t <name> [-w <index>]`
- `dmux split-window -t <name> -h|-v [-- command...]`
- `dmux split -t <name> -h|-v [-- command...]`
- `dmux list-panes -t <name> [-F <format>]`
- `dmux select-pane -t <name> -p <index>`
- `dmux kill-pane -t <name> [-p <index>]`
- `dmux zoom-pane -t <name> [-p <index>]`
- `dmux status-line -t <name> [-F <format>]`
- `dmux display-message -t <name> -p <format>`
- `dmux kill-session -t <name>`
- `dmux kill-server`

`capture-pane -p` defaults to combined scrollback history plus the current
screen. Use `--screen` for only the current screen, `--history` for only
scrollback, or `--all` for the combined output explicitly.

`copy-mode` currently prints command-driven line inspection output as
`<line-number><tab><text>` from the same `--screen`, `--history`, and `--all`
capture sources as `capture-pane`; default and `--all` are combined history plus
screen. Use `--search` to show only matching lines while preserving their
original line numbers.

Attached clients can enter the current basic copy-mode view with `C-b [`.
Inside copy-mode, `j`/`k` and `Ctrl-n`/`Ctrl-p` move the cursor, `y` or Enter
saves the current line to a buffer, and `q` or Escape exits. Mouse click saves
one rendered line; mouse drag saves an inclusive line range. Mouse selection is
currently basic line-level selection. Unzoomed multi-pane attach routes input
and copy-mode to the server active pane and handles `C-b d` to detach and
`C-b o` to cycle the server active pane.

`save-buffer` currently stores captured active-pane text in an in-memory buffer
with a 1 MiB per-buffer limit and a 50-buffer server limit. Use `-b` to name
the buffer, omit `-b` to create an automatic name, and omit `-b` on
`paste-buffer` to paste the latest saved buffer. Selection is command-driven
for now: `--start-line`/`--end-line` saves a 1-based inclusive line range, and
`--search` saves the first matching line.

Implemented Phase 2 groundwork:

- basic terminal screen state for printable shell output
- scrollback-backed `capture-pane -p`
- explicit screen/history/all capture modes for `capture-pane`
- SGR escape stripping in captures
- carriage-return overwrite handling in captures
- explicit PTY resize requests with screen-state resize
- detached session input through `send-keys`
- attach-time PTY resize to the current terminal size when available
- attached clients request PTY resize on terminal `SIGWINCH`
- minimal window tracking with active window selection
- window removal while keeping the session alive
- split-pane sessions with a server-side active pane
- active pane selection by pane index
- pane removal while keeping the session alive
- pane zoom state while keeping all panes alive
- server-side statusline format expansion
- in-memory buffers backed by pane capture and paste into active panes
- command-driven line range and search selection for buffer saves
- command-driven line-numbered copy-mode inspection with search filtering
- attach-time basic copy-mode key handling for line copy
- attach-time basic copy-mode mouse selection for line ranges
- attach-time statusline snapshot rendering
- attach-time split-pane layout snapshot rendering
- polling-based live redraw for multi-pane attach
- active-pane input routing for polling multi-pane attach
- attach-time pane cycling for polling multi-pane attach
- multi-pane attach copy-mode entry for the active pane
- `DEVMUX_ATTACH_SIZE=<cols>x<rows>` override for tests and automation

Current limits:

- multi-pane attach live redraw is polling-based and routes input to the server active pane; numbered pane selection, mouse focus, and composed-layout copy-mode are not implemented yet
- zoomed panes are tracked server-side and keep single-pane live attach behavior
- attach-time statusline rendering is snapshot-only for raw single-pane attach and polled during multi-pane live redraw; event-driven live status redraw is not implemented yet
- in-memory screen and scrollback only
- copy-mode selection is line-based only
- buffer contents are in-memory only
- full terminal protocol support is incomplete
- no named layout or named-window support yet
- Unix/macOS POSIX PTY support only

Supported `send-keys` tokens are literal text plus `Enter`, `Space`, `Tab`,
`Escape`, and `C-c`.
