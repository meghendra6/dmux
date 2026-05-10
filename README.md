# devmux

`dmux` is an early Rust terminal multiplexer prototype.

Implemented Phase 0/1 commands:

- `dmux new -d -s <name> [-- command...]`
- `dmux new -s <name> [-- command...]`
- `dmux attach -t <name>`
- `dmux ls`
- `dmux capture-pane -t <name> -p`
- `dmux resize-pane -t <name> -x <cols> -y <rows>`
- `dmux send-keys -t <name> <keys...>`
- `dmux split-window -t <name> -h|-v [-- command...]`
- `dmux split -t <name> -h|-v [-- command...]`
- `dmux list-panes -t <name>`
- `dmux select-pane -t <name> -p <index>`
- `dmux kill-pane -t <name> [-p <index>]`
- `dmux kill-session -t <name>`
- `dmux kill-server`

Implemented Phase 2 groundwork:

- basic terminal screen state for printable shell output
- scrollback-backed `capture-pane -p`
- SGR escape stripping in captures
- carriage-return overwrite handling in captures
- explicit PTY resize requests with screen-state resize
- detached session input through `send-keys`
- attach-time PTY resize to the current terminal size when available
- attached clients request PTY resize on terminal `SIGWINCH`
- split-pane sessions with a server-side active pane
- active pane selection by pane index
- pane removal while keeping the session alive
- `DEVMUX_ATTACH_SIZE=<cols>x<rows>` override for tests and automation

Current limits:

- split panes are tracked server-side, but layout rendering is not implemented yet
- in-memory screen and scrollback only
- full terminal protocol support is incomplete
- no layout/window support yet
- Unix/macOS POSIX PTY support only

Supported `send-keys` tokens are literal text plus `Enter`, `Space`, `Tab`,
`Escape`, and `C-c`.
