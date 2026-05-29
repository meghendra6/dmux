# dmux Terminal Feature Passthrough Design

## Goal

Make dmux a terminal-protocol-aware multiplexer for the input/key side, not just
a pane splitter. Today the live attach path enters the alternate screen, sets
raw mode, and enables mouse reporting on the outer terminal, but it never
negotiates focus reporting, bracketed paste, or extended keys with the outer
terminal, and it never tells the client which terminal features the active pane
has requested. As a result, modern TUIs running inside a dmux pane (Claude Code,
Codex, vim, fish, etc.) cannot receive inputs they explicitly asked for — most
visibly `Shift+Enter` in Claude Code, which arrives as a plain `CR`.

This design adds a single mechanism — propagate the active pane's requested
terminal features to the attached client, and reconcile the outer terminal to
match — and uses it to deliver three capabilities in PR-sized stages:

1. Bracketed paste passthrough (DECSET 2004)
2. Focus reporting passthrough (DECSET 1004)
3. Extended keys (xterm `modifyOtherKeys` and the kitty keyboard protocol)

## Source-backed motivation

`docs/spec_design.md` §0 states the product thesis directly: dmux must be a
"terminal protocol-aware mux", and Claude Code inside tmux is known to break on
`Shift+Enter`, extended-keys, and notification/progress passthrough unless the
multiplexer negotiates `allow-passthrough`, `extended-keys`, and
`terminal-features`. Fullscreen agent rendering further relies on alternate
screen, mouse capture, OSC 52 clipboard, and synchronized output. dmux already
implements alternate screen, mouse, OSC 52, and synchronized output; the
keyboard/focus/paste negotiation is the remaining gap and the most
differentiating one.

## Current state (verified)

- Inner-app private modes are tracked per pane in
  `term.rs::apply_private_modes`: `1049/1047` (alt screen), `25` (cursor),
  `2004` (bracketed paste), `2026` (synchronized output), `6` (origin). There is
  **no** handling for `1004` (focus), nor for `modifyOtherKeys`
  (`CSI > 4 ; n m`) or the kitty keyboard protocol (`CSI > flags u` push /
  `CSI < u` pop / `CSI ? u` query), because the latter two are `CSI` sequences
  with a `>`/`<` private marker and are not DEC private modes.
- The server already gates behavior on per-pane mode state: `paste-buffer`
  wraps text in `\x1b[200~`/`\x1b[201~` only when
  `Terminal::bracketed_paste_enabled()` is true (`server.rs::paste_buffer_bytes`).
  This is the precedent the live path should follow.
- The live attach render frame is line-framed:
  `FRAME\t<len>\n` then a body of `HEADER_ROWS\t<n>`, `REGIONS\t<count>`,
  one line per region, then `OUTPUT\t<len>` followed by raw bytes. The parser
  (`client.rs::parse_attach_render_frame_body`) currently rejects any bytes
  after the declared `OUTPUT` body, so new metadata must be added as labeled
  lines **before** `OUTPUT`.
- The client sets up the outer terminal with `ENTER_ALTERNATE_SCREEN`
  (`\x1b[?1049h\x1b[?25l`), `ENABLE_MOUSE_MODE` (`\x1b[?1000h\x1b[?1002h\x1b[?1006h`),
  and `RawModeGuard`. Live input is forwarded verbatim by
  `forward_live_snapshot_input`.

## Core mechanism: active-feature propagation

The server's per-pane `Terminal` is the single source of truth for which
features the running program requested. The active pane can change (pane switch,
window switch, session switch) and the running program can toggle features at
any time, so the client must learn the active pane's feature set on every frame.

1. Extend `term.rs` to record the additional requested features per pane:
   - `focus_reporting` (DECSET 1004)
   - `extended_keys` level: `None`, `ModifyOtherKeys(level)`, or
     `Kitty(flags)` — parsed from `CSI > 4 ; n m`, `CSI > flags u`,
     `CSI < u`, and answered for `CSI ? u`.
   - `bracketed_paste` already exists.
2. Add an `ACTIVE_MODES` line to the attach render frame body, emitted after
   `REGIONS` and before `OUTPUT`, encoding the **active pane's** requested
   feature set, e.g.
   `ACTIVE_MODES\tbracketed_paste=1\tfocus=0\textkeys=kitty:5`.
   Parsing is tolerant: unknown keys are ignored and a missing line means "all
   off" so old/new client/server pairs degrade safely.
3. The client keeps a reconciler that diffs the desired outer-terminal feature
   set (derived from the active pane) against what it has currently enabled on
   the outer terminal, and emits only the enable/disable deltas. The reconciler
   reuses the existing mouse-mode reconcile pattern
   (`sync_live_mouse_mode`).
4. On detach/exit, the `RawModeGuard` teardown path disables every feature the
   client enabled, exactly as it already restores mouse mode and the main
   screen.

This mechanism is the shared foundation for all three stages. No stage scrapes
terminal text; everything is driven by explicit DEC/CSI requests already parsed
by the emulator.

## Stages (each is a separate PR)

### Stage 1: Bracketed paste passthrough

- Enable `\x1b[?2004h` on the outer terminal while attached (disable on detach),
  so the OS/terminal wraps real pastes in `\x1b[200~`…`\x1b[201~`.
- In `forward_live_snapshot_input`, if the active pane has bracketed paste
  enabled, forward the markers unchanged; if not, strip the `200~`/`201~`
  markers and forward only the pasted bytes so non-bracketed programs never see
  literal `~` garbage.
- Propagate `bracketed_paste` via `ACTIVE_MODES` (mechanism above).

Acceptance: a pane running a program with `2004` enabled receives bracketed
markers around a paste; a pane without `2004` receives the unwrapped text;
detach restores the outer terminal.

### Stage 2: Focus reporting passthrough

- Track `1004` in `term.rs`; propagate via `ACTIVE_MODES`.
- When the active pane requests focus reporting, enable `\x1b[?1004h` on the
  outer terminal so it emits `\x1b[I` / `\x1b[O`, and forward those to the
  active pane.
- On pane/window/session switch, synthesize a focus-out to the previously
  active pane and a focus-in to the newly active pane when each has focus
  reporting enabled, so per-pane focus tracks the dmux selection rather than the
  outer client's window focus alone.

Acceptance: a focus-aware program (e.g. vim `autoread`, Claude Code) receives
focus-in/out on outer-terminal focus changes and on dmux pane switches; panes
without `1004` receive nothing; detach disables `1004`.

### Stage 3: Extended keys (modifyOtherKeys + kitty keyboard)

- Parse and track the active pane's requested mode: xterm
  `modifyOtherKeys` (`CSI > 4 ; n m`) and the kitty keyboard protocol
  (`CSI > flags u` push, `CSI < u` pop, `CSI ? u` query → reply with the active
  flags). Propagate via `ACTIVE_MODES`.
- Negotiate the matching protocol with the outer terminal only while the active
  pane wants it, and answer outer-terminal queries on behalf of the pane stack.
- Translate key input so modified keys the base encoding cannot express —
  `Shift+Enter`, `Ctrl+Enter`, `Shift+Tab` variants, modified function/arrow
  keys — reach the pane in the encoding it requested. When the active pane has
  no extended-keys request, fall back to today's verbatim forwarding.

Acceptance: with Claude Code in the active pane, `Shift+Enter` inserts a newline
instead of submitting; a plain shell pane is unaffected; detach restores the
outer terminal's keyboard mode.

## Test strategy

Pure, unit-testable pieces (consistent with existing `popup`/`protocol` tests):

- `ACTIVE_MODES` encode/decode round-trip, including tolerance for unknown keys
  and a missing line.
- Bracketed-paste marker forward-vs-strip given an active-pane flag.
- `term.rs` recording of `1004`, `modifyOtherKeys`, and kitty push/pop/query.
- Key-encoding translation table for `modifyOtherKeys` and kitty given a
  requested level.
- Outer-terminal feature reconciler emits only deltas and clears on teardown.

Integration tests (Unix-socket attach, single-threaded), modeled on existing
`tests/phase1_cli.rs` attach tests:

- Paste into a `2004` pane preserves markers; paste into a non-`2004` pane does
  not.
- Focus event reaches a `1004` pane and not a plain pane.
- `Shift+Enter` reaches a pane that requested kitty/modifyOtherKeys as the
  extended encoding, and reaches a plain pane as `CR`.

Full verification per repo convention:

```bash
cargo fmt -- --check
cargo test -- --test-threads=1
git diff --check
```

## Non-goals

- No OSC/DCS output passthrough (notifications, progress, `\ePtmux;…`). That is
  a separate output-side design and a separate plan.
- No terminal-capability auto-detection of the outer terminal beyond enabling
  modes and honoring query replies; assume a modern xterm-class terminal and
  degrade to verbatim forwarding when a feature is not requested.
- No change to copy-mode, layout, or the rendering pipeline beyond adding the
  `ACTIVE_MODES` metadata line to the existing attach render frame.
- No new TUI or terminal dependency.

## Risks

- The attach render frame is on the hot path; the added metadata line must be
  bounded and cheap, and the parser must stay strict about the `OUTPUT` body.
- Extended-keys negotiation is stateful (kitty push/pop is a stack); the active
  pane owns the stack, and pane switches must re-negotiate the outer terminal to
  the new active pane's top-of-stack state.
- Enabling features on the outer terminal must always be undone on detach and on
  abnormal client exit, or the user's shell is left in a modified keyboard mode.
  Reuse the existing `RawModeGuard` teardown ordering.
