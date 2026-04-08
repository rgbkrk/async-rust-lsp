# async-rust-lsp — Build & Development Guide

## Quick start

```bash
cargo build          # compile everything
cargo test           # run all 22 tests (unit + integration + doc)
cargo run            # start the LSP server (reads/writes stdio)
```

## Project layout

```
Cargo.toml                        — workspace manifest
src/
  lib.rs                          — public crate root (re-exports rules)
  main.rs                         — LSP binary (tower-lsp lifecycle)
  rules/
    mod.rs                        — rule registry
    mutex_across_await.rs         — rule + unit tests
tests/
  integration_tests.rs            — fixture-based integration tests
  fixtures/
    bad_mutex_across_await.rs     — patterns that MUST produce diagnostics
    good_no_mutex_across_await.rs — patterns that MUST produce zero diagnostics
```

## Crate structure: lib + bin

The project exposes both a **library** (`async_rust_lsp`) and a **binary** (`async-rust-lsp`):

- The library (`src/lib.rs`) exposes `rules::*` so integration tests can call rule
  functions directly without running the full LSP stack.
- The binary (`src/main.rs`) imports from the lib crate and runs the tower-lsp server.

This split keeps rule logic testable in isolation.

## Adding a new rule

1. Create `src/rules/<rule_name>.rs` with a public `check_<rule_name>(source: &str) -> Vec<Diagnostic>` function.
2. Add `pub mod <rule_name>;` to `src/rules/mod.rs`.
3. Call the function from `Backend::analyze_document` in `src/main.rs`.
4. Add unit tests inside the rule module (follow the pattern in `mutex_across_await.rs`).
5. Add fixture files in `tests/fixtures/` and integration tests in `tests/integration_tests.rs`.

Diagnostic codes must follow the scheme `async-rust/<rule-slug>`.

## Testing a specific rule

```bash
cargo test mutex_across_await     # run only that module's tests
cargo test fixture_good           # run only good-fixture tests
cargo test -- --nocapture         # see println! output
```

## Running with an editor (manual QA)

Build the release binary and point your editor at it.

```bash
cargo build --release
# binary is at: ./target/release/async-rust-lsp
```

### Neovim (nvim-lspconfig)

```lua
local lspconfig = require('lspconfig')
local configs = require('lspconfig.configs')

if not configs.async_rust_lsp then
  configs.async_rust_lsp = {
    default_config = {
      cmd = { '/path/to/async-rust-lsp' },
      filetypes = { 'rust' },
      root_dir = lspconfig.util.root_pattern('Cargo.toml'),
      settings = {},
    },
  }
end

lspconfig.async_rust_lsp.setup {}
```

### VS Code

Add to `.vscode/settings.json`:

```json
{
  "rust-analyzer.server.extraEnv": {},
  "lsp-client.servers": [
    {
      "name": "async-rust-lsp",
      "command": "/path/to/async-rust-lsp",
      "filetypes": ["rust"]
    }
  ]
}
```

Or use the generic **"LSP client"** extension of your choice.

### Zed

Add to `~/.config/zed/settings.json`:

```json
{
  "lsp": {
    "async-rust-lsp": {
      "binary": {
        "path": "/path/to/async-rust-lsp"
      }
    }
  }
}
```

## Claude Code integration

Claude Code automatically consumes LSP diagnostics. Start the server and open a
`.rs` file; warnings will appear in Claude's context as you type.

```bash
# In your project, register the server:
claude lsp add --name async-rust-lsp --command /path/to/async-rust-lsp
```

## Logging

The server writes logs to `$TMPDIR/async-rust-lsp.log` (never to stdout/stderr,
which are reserved for the LSP stdio protocol).

```bash
tail -f /tmp/async-rust-lsp.log
# or macOS:
tail -f "$TMPDIR/async-rust-lsp.log"
```

Control verbosity with `RUST_LOG`:

```bash
RUST_LOG=debug async-rust-lsp
```

## Key dependencies

| Crate | Version | Purpose |
|---|---|---|
| `tower-lsp` | 0.20 | Async LSP server framework (stdio transport) |
| `tree-sitter` | 0.22 | Incremental parser infrastructure |
| `tree-sitter-rust` | 0.21 | Rust grammar for tree-sitter |
| `tokio` | 1 | Async runtime |
| `tracing` + `tracing-appender` | 0.1 / 0.2 | Structured file logging |

## Detection algorithm (mutex-across-await)

The rule walks every `block` node in the tree-sitter AST:

1. **Guard detection** — a `let_declaration` whose `value` field is an
   `await_expression` wrapping a `call_expression` to `.lock()`, `.write()`, or
   `.read()` creates a live guard entry.
2. **Drop detection** — an `expression_statement` of the form `drop(<ident>)` removes
   the named guard from the live set.
3. **Await detection** — any `await_expression` after a live guard's byte offset
   triggers a `WARNING` diagnostic at that `.await` site. This includes awaits in
   the RHS of `let_declaration` nodes (e.g. a second lock acquisition) and awaits
   inside nested blocks (e.g. `if`/`match`/`loop` bodies).
4. **Scope propagation** — outer-scope guards propagate into nested `block` nodes
   (if/match/loop bodies) for await checking. When entering a nested block,
   `drop()` calls and `let` shadowing within that block update a branch-local
   copy of the guard set, so `drop(guard)` in one `if` branch kills liveness
   for subsequent awaits in that branch without affecting the `else` branch.
   Each nested block is also analyzed independently with its own guard list;
   guards defined in inner blocks don't leak to outer blocks.

The rule intentionally does **not** flag `std::sync::Mutex` (sync, no `.await` in
acquisition) — that case is already handled by `clippy::await_holding_lock`.
