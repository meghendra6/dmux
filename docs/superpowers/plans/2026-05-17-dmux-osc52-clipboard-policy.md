# dmux OSC 52 clipboard policy

**Goal:** Add the first explicit clipboard safety policy in the terminal core by blocking OSC 52 clipboard writes from child PTYs by default, while exposing that blocked attempts happened.

**Scope:** This slice implements the default deny path only. It does not add interactive ask/allow UI, host clipboard relay, desktop notification routing, or broader DCS passthrough.

**Architecture:** Filter OSC 52 before bytes reach raw attach clients or raw history. Preserve safe OSC sequences such as OSC 8 for later terminal/render handling. Count blocked clipboard attempts on the pane and expose that metadata through pane/status formats. Keep terminal parsing server-owned and avoid writing mux UI bytes into pane PTYs.

## Planned Work

1. Extend `PtyOutputFilter` to recognize OSC sequences terminated by BEL or ST.
2. Drop OSC 52 sequences by default and report a blocked clipboard count.
3. Preserve non-OSC52 OSC output bytes.
4. Record blocked clipboard attempts on `Pane`.
5. Expose `#{pane.clipboard_blocked}` through `list-panes`, `status-line`, and `display-message`.
6. Document the default policy in README.

## Verification

- Red/green unit tests for filtering complete, split, and incomplete OSC 52 sequences.
- Integration test proving OSC 52 is not present in raw attach history and `#{pane.clipboard_blocked}` is visible.
- Final checks:
  - `cargo fmt --check`
  - `git diff --check origin/main`
  - upload-facing diff keyword scan
  - `cargo test`
  - `cargo install --path .`
  - `/Users/meghendra/.cargo/bin/dmux --help`
