# Devmux Live Input Design

## Goal

Let unzoomed multi-pane `attach` forward user input to the session's active pane
while keeping the polling split-layout redraw path from the previous slice.

This slice removes the current read-only limitation for normal typing. It does
not add pane focus keybindings, multi-pane copy-mode, or an event-driven
compositor.

## Current State

- Single visible pane attach uses the original raw path: the server streams pane
  bytes to the client, and the client writes stdin bytes back to the attach
  socket.
- Unzoomed multi-pane attach returns `OK\tSNAPSHOT\n`; the client enters a
  polling redraw loop using `status-line` and `ATTACH_SNAPSHOT`.
- In the polling redraw loop, stdin is consumed only to detect `C-b d` or EOF.
  Other bytes are ignored.
- The server returns immediately after writing `OK\tSNAPSHOT\n`, so the attach
  socket cannot currently carry multi-pane input.

## Approaches Considered

1. **Add a new raw-input control request:** encode arbitrary bytes in the line
   protocol and send one request per input chunk. This keeps `ATTACH` as a pure
   mode handshake, but it adds protocol surface and byte encoding just to write
   to the active pane.
2. **Keep the multi-pane attach socket open for input:** after
   `OK\tSNAPSHOT\n`, the server reads bytes from the same attach stream and
   forwards each chunk to the current active pane. This mirrors the existing
   single-pane attach input path, keeps older snapshot clients compatible, and
   avoids a new protocol request.
3. **Use the existing `send-keys` command internally:** translate attach bytes
   into tokenized `send-keys` calls. This would lose raw byte fidelity and would
   behave differently from single-pane attach.

## Chosen Design

Use approach 2. The existing attach socket becomes input-only for unzoomed
multi-pane attach:

- server writes `OK\tSNAPSHOT\n`
- server keeps reading from that stream until EOF
- each received byte chunk is written to the session's current active pane
- output remains polling-based through `ATTACH_SNAPSHOT`

The client redraw loop keeps its polling cadence. Its stdin reader sends input
events over an internal channel:

- `C-b d` detaches and is not forwarded
- stdin EOF exits after any pending literal prefix byte is handled
- all other bytes are forwarded to the attach socket
- `C-b C-b` forwards a literal `C-b`
- `C-b` followed by any non-detach byte forwards the prefix and that byte

This deliberately does not implement `C-b [` for multi-pane copy-mode. In this
slice, copy-mode remains available only on single-pane and zoomed raw attach.

## Data Flow

1. Client sends `ATTACH`.
2. Server sees multiple visible panes and writes `OK\tSNAPSHOT\n`.
3. Server enters a read loop on the attach stream.
4. Client enters the polling redraw loop and starts an input thread.
5. Input thread translates stdin into `Forward(bytes)`, `Detach`, or `Eof`.
6. On `Forward(bytes)`, client writes bytes to the attach stream.
7. Server writes those bytes to the currently active pane.
8. Polling redraw picks up resulting pane output through `ATTACH_SNAPSHOT`.
9. On detach or EOF, client shuts down the attach stream; server read loop exits.

## Error Handling

- If the active pane disappears between receiving input and writing it, the
  server stops the attach input loop rather than panicking.
- A write error to the active pane is returned from the server handler and
  closes that attach connection.
- Redraw control request failures still return from attach and exit the client.
- Older snapshot clients remain compatible: they close the attach stream after
  rendering one snapshot, so the server input loop exits cleanly.

## Tests

Add tests before implementation:

- live snapshot input translation forwards arbitrary bytes
- live snapshot input translation detaches on `C-b d`
- live snapshot input translation preserves literal prefix bytes
- unzoomed multi-pane attach forwards typed stdin to the active split pane
- forwarded input becomes visible through the live redraw output
- attach input follows active pane changes made after attach starts
- existing compatibility remains: `OK\tSNAPSHOT\n` handshake, zoomed raw attach,
  and detach behavior still pass

## Out Of Scope

- in-attach pane focus switching
- multi-pane copy-mode entry
- mouse input dispatch to pane regions
- per-pane input targeting from the attach UI
- event-driven redraw or compositor invalidation
- resize-aware split region allocation

## Acceptance Criteria

- A user attached to an unzoomed split session can type into the active pane.
- `C-b d` still detaches and is not written to the pane.
- `C-b C-b` can send a literal prefix byte.
- Multi-pane attach keeps the existing snapshot handshake for older clients.
- Single-pane and zoomed-pane attach behavior is unchanged.
- README documents active-pane input routing and keeps focus switching/copy-mode
  limitations listed as pending.
