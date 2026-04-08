# async-rust-lsp

A standalone LSP server that provides real-time diagnostics for async Rust antipatterns — focusing on patterns that clippy and rust-analyzer miss.

## Problem

Existing Rust tooling has a blind spot for tokio-specific async antipatterns:

- **clippy** only checks `std::sync::Mutex` across `.await`, not `tokio::sync::Mutex`
- **rust-analyzer** has no plugin system for custom diagnostics
- **Runtime tools** (tokio-console) only help during execution, not during editing

This LSP fills the gap with real-time editor feedback for async lock patterns, blocking operations in async contexts, and other tokio-specific footguns.

## Goals

### Diagnostics

- **tokio-mutex-across-await**: Warn when `tokio::sync::Mutex`/`RwLock` guards are held across `.await` points
- **blocking-in-async**: Detect `std::thread::sleep`, `std::fs::*`, and other blocking calls inside async functions
- **nested-lock-ordering**: Flag potential deadlocks from inconsistent lock acquisition order
- **unbounded-channel-in-loop**: Warn about unbounded channel sends in hot loops

### LSP Features

- `textDocument/publishDiagnostics` — real-time warnings as you type
- `textDocument/codeAction` — quick fixes (scope the guard, clone Arc, use `spawn_blocking`)
- Incremental parsing via `tree-sitter-rust` for fast feedback

### Integration

- Works with any LSP-compatible editor (VS Code, Zed, Neovim, Helix)
- **Claude Code** consumes LSP diagnostics automatically — diagnostics appear in agent context
- Configurable via `.async-rust-lsp.toml` per project

## Architecture (aspirational)

```
┌─────────────┐    LSP/stdio    ┌──────────────────┐
│   Editor    │ ◄─────────────► │  async-rust-lsp  │
│ (or Claude) │                 │                  │
└─────────────┘                 │  tree-sitter-rust│
                                │  + custom rules  │
                                └──────────────────┘
```

Built with:
- [`tower-lsp`](https://github.com/ebkalderon/tower-lsp) — async LSP framework
- [`tree-sitter-rust`](https://github.com/tree-sitter/tree-sitter-rust) — incremental Rust parser
- Custom rule engine for async pattern matching

## Origin

Born from a real deadlock in the [nteract desktop](https://github.com/nteract/desktop) daemon. See [nteract/desktop#1614](https://github.com/nteract/desktop/pull/1614). The gap in static analysis for tokio async patterns is well-known but no one has filled it with an LSP yet.

## License

MIT
