# async-rust-lsp

A standalone LSP server that provides real-time diagnostics for async Rust antipatterns — focusing on patterns that clippy and rust-analyzer miss.

## Problem

Existing Rust tooling has a blind spot for tokio-specific async antipatterns:

- **clippy** only checks `std::sync::Mutex` across `.await`, not `tokio::sync::Mutex`
- **rust-analyzer** has no plugin system for custom diagnostics
- **Runtime tools** (tokio-console) only help during execution, not during editing

This LSP fills the gap with real-time editor feedback for async lock patterns.

## Current rules

### `async-rust/mutex-across-await`

Warns when `tokio::sync::Mutex` or `RwLock` guards are held across `.await` points — a pattern that can deadlock under tokio's cooperative scheduling.

```rust
// BAD — guard lives across the await
let guard = mutex.lock().await;
do_something(&guard);
some_future.await; // WARNING: deadlock risk

// OK — guard scoped before the await
let value = {
    let guard = mutex.lock().await;
    guard.clone()
};
some_future.await; // fine
```

The rule tracks guard liveness through:
- `drop(guard)` calls (including inside conditional branches)
- `let` shadowing that kills the guard binding
- Block scoping (guard dropped at end of block)

### `async-rust/cancel-unsafe-in-select`

Warns when a non-cancel-safe future is used in the future-expression position of a `tokio::select!` arm. When a sibling arm wins the race, losing futures are dropped mid-poll — futures built on `read_exact`, `write_all`, and friends discard their buffered bytes when dropped, silently desynchronizing length-prefixed wire protocols.

```rust
// BAD — read_exact loses bytes when the other arm wins
tokio::select! {
    _ = reader.read_exact(&mut buf) => (), // WARNING: not cancel-safe
    _ = sleep_for_a_bit() => (),
}

// OK — mpsc::Receiver::recv is cancel-safe; move the read into an actor
tokio::select! {
    msg = framed_reader.recv() => (), // recv() over mpsc — fine to drop
    _ = sleep_for_a_bit() => (),
}
```

Flagged tokio primitives:
- `read_exact`, `read_to_end`, `read_to_string`, `read_buf`
- `read_line`, `read_until`
- `write_all`, `write_buf`, `write_all_buf`

The rule only flags calls in the *future-expression* position (LHS of `=>`). Calls inside an arm's handler block are fine — by the time the block runs, the arm has already won and won't be cancelled.

**Project-local wrappers** — drop a `.async-rust-lsp.toml` next to your `Cargo.toml`:

```toml
[rules.cancel-unsafe-in-select]
extra = ["recv_typed_frame", "send_typed_frame"]
```

The rule can't follow function bodies across files, so a wrapper that internally calls `read_exact` won't be flagged by default. List the wrapper names in `extra` and the rule will treat them like the built-in primitives. Built-in names listed in `extra` are deduplicated.

Origin: nteract relay desync that surfaced as `frame too large: 1818192238 bytes` — four bytes of streaming kernel stdout reinterpreted as a length prefix. See [nteract/desktop#2182](https://github.com/nteract/desktop/pull/2182).

## Installation

```bash
cargo install --git https://github.com/rgbkrk/async-rust-lsp
```

Or build from source:

```bash
git clone https://github.com/rgbkrk/async-rust-lsp
cd async-rust-lsp
cargo build --release
# binary at ./target/release/async-rust-lsp
```

## Editor setup

### Neovim (nvim-lspconfig)

```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

if not configs.async_rust_lsp then
  configs.async_rust_lsp = {
    default_config = {
      cmd = { 'async-rust-lsp' },
      filetypes = { 'rust' },
      root_dir = lspconfig.util.root_pattern('Cargo.toml'),
    },
  }
end

lspconfig.async_rust_lsp.setup {}
```

### VS Code

Use a generic LSP client extension and add to `.vscode/settings.json`:

```json
{
  "lsp-client.servers": [
    {
      "name": "async-rust-lsp",
      "command": "async-rust-lsp",
      "filetypes": ["rust"]
    }
  ]
}
```

### Zed

Add to `~/.config/zed/settings.json`:

```json
{
  "lsp": {
    "async-rust-lsp": {
      "binary": {
        "path": "async-rust-lsp"
      }
    }
  }
}
```

### Claude Code

```bash
claude lsp add --name async-rust-lsp --command async-rust-lsp
```

Diagnostics appear automatically in Claude's context when editing `.rs` files.

## Logging

The server logs to `$TMPDIR/async-rust-lsp.log` (never stdout/stderr, which are reserved for LSP stdio protocol).

```bash
tail -f "$TMPDIR/async-rust-lsp.log"
RUST_LOG=debug async-rust-lsp  # verbose logging
```

## Architecture

```
┌─────────────┐    LSP/stdio    ┌──────────────────┐
│   Editor    │ <─────────────> │  async-rust-lsp  │
│ (or Claude) │                 │                  │
└─────────────┘                 │  tree-sitter-rust│
                                │  + custom rules  │
                                └──────────────────┘
```

Built with:
- [`tower-lsp`](https://github.com/ebkalderon/tower-lsp) — async LSP framework
- [`tree-sitter-rust`](https://github.com/tree-sitter/tree-sitter-rust) — incremental Rust parser

## Origin

Born from a real deadlock in the [nteract desktop](https://github.com/nteract/desktop) daemon. See [nteract/desktop#1614](https://github.com/nteract/desktop/pull/1614). The gap in static analysis for tokio async patterns is well-known but no one has filled it with an LSP yet.

## License

MIT
