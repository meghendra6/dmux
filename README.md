# devmux

`dmux` is an early Rust terminal multiplexer prototype.

Implemented Phase 0/1 commands:

- `dmux new -d -s <name> [-- command...]`
- `dmux new -s <name> [-- command...]`
- `dmux attach -t <name>`
- `dmux ls`
- `dmux capture-pane -t <name> -p`
- `dmux resize-pane -t <name> -x <cols> -y <rows>`
- `dmux kill-session -t <name>`
- `dmux kill-server`

Implemented Phase 2 groundwork:

- basic terminal screen state for printable shell output
- scrollback-backed `capture-pane -p`
- SGR escape stripping in captures
- carriage-return overwrite handling in captures
- explicit PTY resize requests with screen-state resize

Current limits:

- single pane per session
- in-memory screen and scrollback only
- full terminal protocol support is incomplete
- no layout/window support yet
- Unix/macOS POSIX PTY support only
