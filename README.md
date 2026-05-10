# devmux

`dmux` is an early Rust terminal multiplexer prototype.

Implemented Phase 0/1 commands:

- `dmux new -d -s <name> [-- command...]`
- `dmux new -s <name> [-- command...]`
- `dmux attach -t <name>`
- `dmux ls`
- `dmux capture-pane -t <name> -p [--screen|--history|--all]`
- `dmux save-buffer -t <name> [-b <buffer>] [--screen|--history|--all]`
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

`save-buffer` currently stores captured active-pane text in an in-memory buffer
with a 1 MiB per-buffer limit and a 50-buffer server limit. Use `-b` to name
the buffer, omit `-b` to create an automatic name, and omit `-b` on
`paste-buffer` to paste the latest saved buffer.

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
- `DEVMUX_ATTACH_SIZE=<cols>x<rows>` override for tests and automation

Current limits:

- split panes are tracked server-side, but layout rendering is not implemented yet
- zoomed panes are tracked server-side, but layout rendering is not implemented yet
- statusline format expansion is implemented, but attach-time statusline rendering is not implemented yet
- in-memory screen and scrollback only
- buffer contents are in-memory only and are not yet fed by interactive selection
- full terminal protocol support is incomplete
- no layout or named-window support yet
- Unix/macOS POSIX PTY support only

Supported `send-keys` tokens are literal text plus `Enter`, `Space`, `Tab`,
`Escape`, and `C-c`.
