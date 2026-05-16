# Devmux Terminal Renderer Architecture Design

## Goal

Replace the live multi-pane attach architecture that redraws server-side plain
text snapshots with a terminal-renderer architecture suitable for real terminal
multiplexer use.

The user-visible target is simple: `dmux new -s test`, `C-b %`, shell prompts,
colors, cursor behavior, pane focus, pane output, resize, and detach should feel
like one coherent TUI. It should not look like an old captured shell and should
not refresh a full text snapshot while idle.

## Diagnosis

The current implementation has two fundamentally different attach modes:

- Single visible pane attach is byte-stream based. The server sends raw pane
  PTY bytes to the client and forwards client input to the active pane.
- Multi-pane attach switches to snapshot mode. `ATTACH` returns
  `OK\tSNAPSHOT\n`; the client repeatedly reads `STATUS_LINE` and
  `ATTACH_LAYOUT_SNAPSHOT`, clears the screen, and prints a composed text
  snapshot.

Snapshot mode was useful as a small foundation for split layout tests, but it is
not a correct live multiplexer UI. It renders `TerminalState::capture_screen_text`
instead of terminal cells with styles, so SGR colors, prompt styling, cursor
shape, alternate-screen state, and many terminal semantics are either lost or
approximated away. It also creates an architectural temptation to fix visual
bugs with redraw timing tweaks instead of fixing the renderer boundary.

## Reference Lessons From A Mature Multiplexer

A mature terminal multiplexer does not treat multi-pane attach as "capture text
and repaint it." The important shape is:

- `PtyBus` reads one PTY stream per terminal pane and sends PTY bytes to the
  screen subsystem.
- `ScreenInstruction::PtyBytes` routes bytes to the tab/pane that owns the PTY.
- `TerminalPane` owns a `Grid` of `TerminalCharacter` values, where each cell
  carries a character plus style.
- ANSI/VT parsing updates that grid. Serialization exists, but live rendering
  is based on changed styled cells/chunks, not plain capture text.
- `Screen::render()` schedules a debounced `RenderToClients`; the render pass
  sends `ServerToClientMsg::Render { content }`.
- The client loop receives `ClientInstruction::Render(output)` and writes that
  output to stdout while separately handling input, terminal size queries, and
  Unix signals such as `SIGWINCH`.

The dmux design does not need a plugin system, sixel pipeline, watcher clients,
or full layout engine. It does need the same boundary: live UI renders from
terminal cell state, and the client writes render output from an event stream.
Plain text capture must remain an explicit command/debug path, not the live
attach renderer.

## Target Architecture

### 1. Terminal Model

`src/term.rs` should stop being a plain-character capture helper for live UI.
It should become the pane terminal model:

```text
TerminalState
  - primary grid: rows of Cell
  - optional alternate grid
  - scrollback for capture/copy-mode
  - cursor position, shape, visibility
  - current style
  - dirty rows or dirty cells

Cell
  - char
  - style: fg, bg, bold, dim, italic, underline, inverse
```

Initial scope should support the terminal semantics that explain the current
bug before chasing completeness:

- SGR reset and common style attributes;
- 8/16 color, 256 color, and truecolor foreground/background;
- cursor movement already present today;
- clear screen/line already present today;
- alternate screen enter/exit per pane;
- cursor visibility and shape if cheap to preserve.

`capture-pane` continues to return plain text by projecting the grid into text.
Live rendering does not use `capture_screen_text()`.

### 2. Server Render State

The server remains the owner of pane PTYs and layout. It should also own the
composed render frame for attached clients.

On pane output:

1. The output pump reads bytes from the pane PTY.
2. It appends raw history for raw/capture compatibility.
3. It applies bytes to the pane `TerminalState`.
4. It invalidates render state for the session/window.
5. A debounced render pass composes a frame for attached clients.

The current layout tree and pane regions remain the source of truth. Rendering
uses those regions to crop each pane's styled grid into the correct rectangle.
Pane separators should be minimal and deterministic; avoid decorative padding.

### 3. Render Protocol

Add a new live attach protocol for multi-pane UI. The exact wire format can be
small, but it must be a push stream, not repeated control requests.

Proposed first version:

```text
ATTACH_RENDER\t<session>\n
OK\tRENDER\n
FRAME\t<len>\n
<terminal escape output bytes>
FRAME\t<len>\n
<terminal escape output bytes>
...
```

The frame body is already encoded terminal output for the outer client terminal:
cursor moves, SGR style changes, text chunks, status row, separators, cursor
placement, and optional clear operations.

This keeps the client small and matches dmux's current dependency-free style.
The first version should not introduce a structured cell/chunk serialization
framework; if encoded terminal output proves insufficient, that should be a
separate design change with its own tests.

Compatibility:

- Keep `ATTACH_LAYOUT_SNAPSHOT` for tests, `capture-pane`, debugging, and
  old-client fallback.
- Multi-pane live attach should prefer `ATTACH_RENDER`.
- Snapshot attach should become a fallback path only, not the primary live UI.

### 4. Client Attach Loop

The multi-pane client loop should become a normal TUI event loop:

- enter raw mode;
- enter alternate screen;
- hide/show cursor around render as needed;
- read stdin and translate `C-b` bindings;
- send pane input/control commands to the server;
- read pushed render frames and write them directly to stdout;
- handle resize as an event and send resize to the server immediately;
- restore terminal state on detach, EOF, server exit, and signal.

The client should not poll `STATUS_LINE` or `ATTACH_LAYOUT_SNAPSHOT` while live
attached. It may keep a watchdog only to detect a dead server/stream.

### 5. Status Line And Help

Status/help should be part of the composed render frame, not a separate line
printed before a captured pane snapshot.

- Reserve rows before calculating pane regions.
- Normal mode: one status row.
- Transient help/pane-number messages: render in a reserved row or replace the
  status row temporarily.
- The renderer must never emit more rows than the attach PTY height.

### 6. Copy Mode

Copy mode can stay composed and server-assisted, but it should render from the
same terminal model:

- copy/capture uses plain text projection from grids;
- live copy-mode UI uses render frames;
- entering copy mode pauses normal pane input and normal live frames for that
  client, not the entire session.

## Migration Plan

### Phase 0: Guardrails Already Added

Keep the current real PTY tests:

- split from interactive `dmux new` creates independent usable panes;
- idle snapshot attach does not spam full redraws;
- snapshot attach enters/restores alternate screen;
- frame output fits PTY rows;
- control commands have timeouts so tests cannot hang forever.

These tests are not the final renderer tests, but they prevent regression while
the renderer is replaced.

### Phase 1: Styled Terminal Cells

Extend `TerminalState` with styled cells while preserving current plain capture
APIs. Add unit tests before implementation:

- SGR red/green/bold/reset updates cell style;
- plain capture does not include SGR escapes;
- render projection does include SGR when written to a client frame;
- alternate-screen bytes update alternate grid without destroying primary
  screen;
- cursor movement plus styled output lands in the expected cells.

### Phase 2: Frame Composer

Add a server-side `RenderFrame` composer:

- input: session/window layout, pane regions, status/message state, terminal
  styled grids;
- output: terminal escape bytes for the outer attach terminal;
- no control requests during render;
- no full redraw when there are no dirty cells or dirty status.

Tests:

- horizontal split frame preserves color escapes from both panes;
- vertical split frame preserves cursor-relative content;
- status row plus panes never exceeds PTY height;
- unchanged idle frame emits no bytes or no frame.

### Phase 3: Push Render Attach

Add `ATTACH_RENDER` and server render subscriptions.

Tests:

- attach render handshake returns `OK\tRENDER`;
- pane output pushes a frame without a polling command;
- select/split/kill/resize pushes a frame;
- dead render clients are removed without blocking pane output.

### Phase 4: Switch Multi-Pane Live Attach

Make client multi-pane attach use `ATTACH_RENDER`.

Tests:

- `dmux new -s test`, `C-b %` preserves colored prompt output in both panes;
- input goes only to the active pane;
- `C-b h/j/k/l/o/q/x/z/?/d` still work;
- detach restores terminal modes;
- killing session/server exits attached clients without hangs.

### Phase 5: Demote Snapshot Attach

After render attach passes the live tests:

- keep snapshot endpoints only for compatibility/debug;
- remove snapshot polling from the normal attach path;
- update README and older design docs to mark snapshot attach as historical
  scaffolding.

## Acceptance Criteria

- Multi-pane attach does not use repeated `STATUS_LINE` +
  `ATTACH_LAYOUT_SNAPSHOT` polling for live UI.
- Colored prompts and common SGR styling survive split and redraw.
- Idle attach produces no visible refresh.
- The client terminal is always restored after detach, server exit, and killed
  sessions.
- Full test suite passes, including real PTY integration tests that cover
  colors, split, focus, resize, and detach.

## Non-Goals

- Copy reference code.
- Add plugins, complex layout systems, sixel images, web clients, or watcher
  clients.
- Perfect terminal emulation in one PR.
- Large new dependencies unless a specific terminal parser/renderer is chosen
  deliberately in a separate design update.
