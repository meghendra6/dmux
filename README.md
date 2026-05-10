# devmux

`dmux` is an early Rust terminal multiplexer prototype.

Implemented Phase 0/1 commands:

- `dmux new -d -s <name> [-- command...]`
- `dmux new -s <name> [-- command...]`
- `dmux attach -t <name>`
- `dmux ls`
- `dmux capture-pane -t <name> -p`
- `dmux kill-session -t <name>`
- `dmux kill-server`

Current limits:

- single pane per session
- in-memory scrollback only
- no layout/window support yet
- Unix/macOS POSIX PTY support only
