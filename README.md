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
